//! **TTX3**——万级大纹理档案：瓦片化 + 层级混搭 + 流式瓦片解码。
//!
//! 目标（与 TTX2 的差异）：
//! - **万级分辨率**：基面 10240×10240 = 1.05 亿纹素（[`MAX_LEVEL_PIXELS`]
//!   允许到 2^30）；TTX2 的 [`crate::MAX_TEXTURE_PIXELS`] 上限是它的 1/25；
//! - **瓦片化**：纹理切成 [`TILE_WIDTH`]×[`TILE_HEIGHT`] 的瓦片
//!   （256×320 纹素 = 64×32 个 4×10 块，与块形状严丝合缝），
//!   瓦片是编码 / 校验 / 解码 / 显存上传的**统一单元**；
//! - **层级混搭**：每级一个 [`LevelPolicy`]——细层 `Capacity`（2.4 bpp 纯容量），
//!   粗层 `Strength`（瓦片**双写冗余**：粗层字节数小、单副本损坏自动恢复，
//!   强度收益/代价比最高）；
//! - **流式**：[`Ttx3Archive::decode_tile`] 只物化一个瓦片（320 KB RGBA），
//!   全档不必驻留内存；瓦片是显存上传单元；
//! - **兼容**：逐瓦片 GF(3) CRC-16（沿用 TTX2「按内容自己的基数算」的哲学）；
//!   u64 偏移；未知 flags / 未知政策字节显式拒绝（不猜）。
//!
//! 边界（M0）：级尺寸必须是瓦片尺寸的整数倍（[`suggest_chain`] 负责把标准
//! 减半链吸附到合法尺寸）；不做部分瓦片；不自动生成 mip（级是显式给的）。

use crate::ttx::{block_trits, gf3_remainder_16};
use crate::{
    BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_BLOCK_BYTES, TRITS_PER_BLOCK, TextureError, TritBlock,
    TritTexture, decode_rgba_into,
};

/// 容器魔数。
pub const MAGIC: [u8; 4] = *b"TTX3";
/// 头部字节数：magic(4) + flags(u16) + reserved(u16) + level_count(u8) +
/// reserved\[3\] + levels_dir_offset(u64) = 20。
pub const HEADER_BYTES: usize = 20;
/// 级目录条目字节数：w(u32) + h(u32) + policy(u8) + reserved\[3\] +
/// tiles_dir_offset(u64)。
pub const LEVEL_ENTRY_BYTES: usize = 20;
/// 瓦片目录条目字节数：data_offset(u64) + copy2_offset(u64) + len(u32) +
/// gf3_crc16(u32)。
pub const TILE_ENTRY_BYTES: usize = 24;
/// 单边边长上限。
pub const MAX_SIDE: usize = 1 << 16;
/// 单级纹素上限（2^30；万级 10240² = 1.05 亿，还有 10 倍余量）。
pub const MAX_LEVEL_PIXELS: usize = 1 << 30;
/// 级数上限（65536 半减到瓦片级约 9 级，留一倍余量）。
pub const MAX_LEVELS: usize = 18;
/// 瓦片宽（64 个 4 宽块）。
pub const TILE_WIDTH: usize = 256;
/// 瓦片高（32 个 10 高块）。
pub const TILE_HEIGHT: usize = 320;
/// 每瓦片纹素数。
pub const TILE_TEXELS: usize = TILE_WIDTH * TILE_HEIGHT;
/// 每瓦片 4×10 块数。
pub const TILE_BLOCKS: usize = TILE_TEXELS / (BLOCK_WIDTH * BLOCK_HEIGHT);
/// 每瓦片数据字节数。
pub const TILE_DATA_BYTES: usize = TILE_BLOCKS * TRIT_BLOCK_BYTES;

/// 级政策：层级混搭的两种档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelPolicy {
    /// 容量档：单份瓦片，2.4 bpp 纯容量（给细层——字节数大头）。
    Capacity,
    /// 强度档：瓦片**双写冗余**，每份独立 GF(3) CRC-16，单副本损坏自动恢复
    /// （给粗层——字节数小、抗灾收益大）。
    Strength,
}

impl LevelPolicy {
    fn from_byte(byte: u8) -> Result<Self, TextureError> {
        match byte {
            0 => Ok(Self::Capacity),
            1 => Ok(Self::Strength),
            other => Err(TextureError::Ttx3Level {
                level: other as usize,
                message: format!("未知级政策字节 {other}（只认 0=Capacity 1=Strength）"),
            }),
        }
    }

    fn to_byte(self) -> u8 {
        match self {
            Self::Capacity => 0,
            Self::Strength => 1,
        }
    }

    /// 该档的瓦片冗余份数。
    pub fn copies(self) -> usize {
        match self {
            Self::Capacity => 1,
            Self::Strength => 2,
        }
    }
}

/// 一级：尺寸 + 政策 + 瓦片（行主序）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Level {
    /// 级宽（瓦片宽的倍数）。
    pub width: usize,
    /// 级高（瓦片高的倍数）。
    pub height: usize,
    /// 该级政策。
    pub policy: LevelPolicy,
    /// 瓦片（行主序，`tiles_x × tiles_y` 个，每片 [`TILE_BLOCKS`] 个块）。
    pub tiles: Vec<Tile>,
}

/// 一个瓦片的块数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tile {
    /// [`TILE_BLOCKS`] 个 8×8 块（行主序，块内纹素按 TT3 规则排布）。
    pub blocks: Vec<TritBlock>,
}

/// 内存中的万级大纹理（编码侧）：按瓦片存储，逐级填充。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LargeTexture {
    /// 基面宽（瓦片宽的倍数）。
    pub width: usize,
    /// 基面高（瓦片高的倍数）。
    pub height: usize,
    /// 从细到粗的各级。
    pub levels: Vec<Level>,
}

/// 标准减半链 + 默认混搭：宽高各减半并向下吸附到瓦片倍数，直到瓦片尺寸；
/// **最粗两级 `Strength`，其余 `Capacity`**——这就是默认的层级混搭法。
pub fn suggest_chain(base_width: usize, base_height: usize) -> Vec<(usize, usize, LevelPolicy)> {
    let mut chain = Vec::new();
    let (mut w, mut h) = (base_width, base_height);
    loop {
        chain.push((w, h));
        let next_w = (w / 2 / TILE_WIDTH * TILE_WIDTH).max(TILE_WIDTH);
        let next_h = (h / 2 / TILE_HEIGHT * TILE_HEIGHT).max(TILE_HEIGHT);
        if (next_w, next_h) == (w, h) {
            break;
        }
        (w, h) = (next_w, next_h);
    }
    let n = chain.len();
    chain
        .into_iter()
        .enumerate()
        .map(|(index, (w, h))| {
            let policy = if index + 2 >= n {
                LevelPolicy::Strength
            } else {
                LevelPolicy::Capacity
            };
            (w, h, policy)
        })
        .collect()
}

fn validate_dims(width: usize, height: usize) -> Result<(), TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::Ttx3Dims {
            width,
            height,
            reason: "宽高必须非零".into(),
        });
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(TextureError::Ttx3Dims {
            width,
            height,
            reason: format!("边长超过上限 {MAX_SIDE}"),
        });
    }
    if !width.is_multiple_of(TILE_WIDTH) || !height.is_multiple_of(TILE_HEIGHT) {
        return Err(TextureError::Ttx3Dims {
            width,
            height,
            reason: format!("宽必须是 {TILE_WIDTH} 的倍数、高必须是 {TILE_HEIGHT} 的倍数"),
        });
    }
    if width * height > MAX_LEVEL_PIXELS {
        return Err(TextureError::Ttx3Dims {
            width,
            height,
            reason: format!("纹素超过单级上限 {MAX_LEVEL_PIXELS}"),
        });
    }
    Ok(())
}

fn tile_grid(width: usize, height: usize) -> (usize, usize) {
    (width / TILE_WIDTH, height / TILE_HEIGHT)
}

/// 级的存档字节账（瓦片目录 + 瓦片数据 × 冗余）。
pub fn level_storage_bytes(width: usize, height: usize, policy: LevelPolicy) -> u64 {
    let (tx, ty) = tile_grid(width, height);
    let tiles = (tx * ty) as u64;
    tiles * (TILE_DATA_BYTES as u64 * policy.copies() as u64 + TILE_ENTRY_BYTES as u64)
}

impl LargeTexture {
    /// 新建：`policies` 从细到粗给出每级政策；级尺寸按标准减半链推导。
    pub fn new(
        base_width: usize,
        base_height: usize,
        policies: &[LevelPolicy],
    ) -> Result<Self, TextureError> {
        validate_dims(base_width, base_height)?;
        if policies.is_empty() || policies.len() > MAX_LEVELS {
            return Err(TextureError::Ttx3Level {
                level: policies.len(),
                message: format!("级数必须在 1..={MAX_LEVELS}"),
            });
        }
        let mut levels = Vec::with_capacity(policies.len());
        let (mut w, mut h) = (base_width, base_height);
        for (index, policy) in policies.iter().enumerate() {
            validate_dims(w, h)?;
            if index > 0 {
                let previous: &Level = &levels[index - 1];
                if (w, h) == (previous.width, previous.height) {
                    return Err(TextureError::Ttx3Level {
                        level: index,
                        message: format!(
                            "第 {index} 级已减无可减（瓦片尺寸 {TILE_WIDTH}×{TILE_HEIGHT}）"
                        ),
                    });
                }
            }
            let (tx, ty) = tile_grid(w, h);
            levels.push(Level {
                width: w,
                height: h,
                policy: *policy,
                tiles: vec![
                    Tile {
                        blocks: vec![[0u8; TRIT_BLOCK_BYTES]; TILE_BLOCKS]
                    };
                    tx * ty
                ],
            });
            (w, h) = (
                (w / 2 / TILE_WIDTH * TILE_WIDTH).max(TILE_WIDTH),
                (h / 2 / TILE_HEIGHT * TILE_HEIGHT).max(TILE_HEIGHT),
            );
        }
        Ok(Self {
            width: base_width,
            height: base_height,
            levels,
        })
    }

    /// 把一级的 RGBA 像素编码进去（逐瓦片编码，峰值内存 = 单瓦片）。
    pub fn push_level_rgba(&mut self, level: usize, rgba: &[u8]) -> Result<(), TextureError> {
        let (width, height) = match self.levels.get(level) {
            Some(level_data) => (level_data.width, level_data.height),
            None => {
                return Err(TextureError::Ttx3Level {
                    level,
                    message: "级不存在".into(),
                });
            }
        };
        if rgba.len() != width * height * 4 {
            return Err(TextureError::SizeMismatch {
                actual: rgba.len(),
                expected: width * height * 4,
            });
        }
        let tiles_x = width / TILE_WIDTH;
        let blocks_per_row = TILE_WIDTH / BLOCK_WIDTH;
        for tile_index in 0..self.levels[level].tiles.len() {
            let tx = tile_index % tiles_x;
            let ty = tile_index / tiles_x;
            let tile = &mut self.levels[level].tiles[tile_index];
            for (block, slot) in tile.blocks.iter_mut().enumerate() {
                let bx = (block % blocks_per_row) * BLOCK_WIDTH;
                let by = (block / blocks_per_row) * BLOCK_HEIGHT;
                let mut texels = [[0u8; 4]; BLOCK_WIDTH * BLOCK_HEIGHT];
                for (i, texel) in texels.iter_mut().enumerate() {
                    let px = tx * TILE_WIDTH + bx + i % BLOCK_WIDTH;
                    let py = ty * TILE_HEIGHT + by + i / BLOCK_WIDTH;
                    let src = (py * width + px) * 4;
                    *texel = [rgba[src], rgba[src + 1], rgba[src + 2], rgba[src + 3]];
                }
                *slot = crate::encode_block(&texels);
            }
        }
        Ok(())
    }

    /// 解码某一级为完整 RGBA（整级物化；流式请用 [`Ttx3Archive::decode_tile`]）。
    ///
    /// 块按**纹理块行主序**从瓦片里交错收集：TritTexture 的块序是整图
    /// 行主序，而瓦片按瓦片存储——直接顺序拼接会把瓦片(1,0)的第一块行
    /// 排到瓦片(0,0)整片之后（自审时的对拍抓过）。
    pub fn decode_level(&self, level: usize) -> Result<Vec<u8>, TextureError> {
        let level_data = self.levels.get(level).ok_or(TextureError::Ttx3Level {
            level,
            message: "级不存在".into(),
        })?;
        let (tiles_x, _) = tile_grid(level_data.width, level_data.height);
        let tile_block_cols = TILE_WIDTH / BLOCK_WIDTH;
        let tile_block_rows = TILE_HEIGHT / BLOCK_HEIGHT;
        let mut texture = TritTexture {
            width: level_data.width,
            height: level_data.height,
            blocks: Vec::with_capacity(level_data.tiles.len() * TILE_BLOCKS),
        };
        for block_row in 0..level_data.height / BLOCK_HEIGHT {
            let ty = block_row / tile_block_rows;
            let row_in_tile = block_row % tile_block_rows;
            for block_col in 0..level_data.width / BLOCK_WIDTH {
                let tx = block_col / tile_block_cols;
                let col_in_tile = block_col % tile_block_cols;
                let tile = &level_data.tiles[ty * tiles_x + tx];
                texture
                    .blocks
                    .push(tile.blocks[row_in_tile * tile_block_cols + col_in_tile]);
            }
        }
        let mut out = vec![0u8; texture.width * texture.height * 4];
        decode_rgba_into(&texture, &mut out)?;
        Ok(out)
    }

    /// 序列化为 TTX3 字节流。布局全部可封闭计算（无需回填）：
    /// 头部 → 级目录 → 依次每级（瓦片目录 → 瓦片数据 × 冗余份数）。
    pub fn to_bytes(&self) -> Vec<u8> {
        let level_count = self.levels.len();
        let dir_offsets: Vec<u64> = {
            let mut offsets = Vec::with_capacity(level_count);
            let mut cursor = HEADER_BYTES as u64 + level_count as u64 * LEVEL_ENTRY_BYTES as u64;
            for level in &self.levels {
                offsets.push(cursor);
                let tiles = level.tiles.len() as u64;
                cursor += tiles * TILE_ENTRY_BYTES as u64
                    + tiles * TILE_DATA_BYTES as u64 * level.policy.copies() as u64;
            }
            offsets
        };
        let mut out = Vec::with_capacity(
            *dir_offsets.last().unwrap() as usize
                + self.levels.last().map_or(0, |l| {
                    l.tiles.len() * (TILE_ENTRY_BYTES + TILE_DATA_BYTES * l.policy.copies())
                }),
        );
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // reserved
        out.push(level_count as u8);
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(&(HEADER_BYTES as u64).to_le_bytes());
        for (index, level) in self.levels.iter().enumerate() {
            out.extend_from_slice(&(level.width as u32).to_le_bytes());
            out.extend_from_slice(&(level.height as u32).to_le_bytes());
            out.push(level.policy.to_byte());
            out.extend_from_slice(&[0u8; 3]);
            out.extend_from_slice(&dir_offsets[index].to_le_bytes());
        }
        for (index, level) in self.levels.iter().enumerate() {
            let dir_start = dir_offsets[index] as usize;
            let tiles = level.tiles.len();
            for (tile_index, tile) in level.tiles.iter().enumerate() {
                let mut tile_bytes = Vec::with_capacity(TILE_DATA_BYTES);
                for block in &tile.blocks {
                    tile_bytes.extend_from_slice(block);
                }
                let crc = tile_crc(&tile_bytes);
                let data_start = dir_start + tiles * TILE_ENTRY_BYTES;
                let off1 = data_start as u64 + tile_index as u64 * TILE_DATA_BYTES as u64;
                let off2 = if level.policy == LevelPolicy::Strength {
                    off1 + TILE_DATA_BYTES as u64
                } else {
                    0
                };
                out.extend_from_slice(&off1.to_le_bytes());
                out.extend_from_slice(&off2.to_le_bytes());
                out.extend_from_slice(&(TILE_DATA_BYTES as u32).to_le_bytes());
                out.extend_from_slice(&crc.to_le_bytes());
            }
            for tile in &level.tiles {
                for block in &tile.blocks {
                    out.extend_from_slice(block);
                }
                if level.policy == LevelPolicy::Strength {
                    for block in &tile.blocks {
                        out.extend_from_slice(block);
                    }
                }
            }
        }
        out
    }
}

/// 瓦片解码结果：`TILE_WIDTH × TILE_HEIGHT × 4` 的 RGBA（320 KB）。
pub type TileRgba = Vec<u8>;

/// 瓦片校验：GF(3) CRC-16 盖**三元载荷**（按内容自己的基数，与 TTX2
/// 同源），RGB565 端点字节是二进制内容，用 FNV-1a 折进同一个 u32——
/// 只盖载荷的话端点损坏检不出来（自审发现）。
fn tile_crc(slice: &[u8]) -> u32 {
    let mut trits = Vec::with_capacity(slice.len() / TRIT_BLOCK_BYTES * TRITS_PER_BLOCK);
    let mut fnv: u32 = 0x811c_9dc5;
    for block_index in 0..slice.len() / TRIT_BLOCK_BYTES {
        let mut block = [0u8; TRIT_BLOCK_BYTES];
        block.copy_from_slice(
            &slice[block_index * TRIT_BLOCK_BYTES..(block_index + 1) * TRIT_BLOCK_BYTES],
        );
        trits.extend_from_slice(&block_trits(&block));
        for &byte in &block[..4] {
            fnv ^= byte as u32;
            fnv = fnv.wrapping_mul(0x0100_0193);
        }
    }
    u32::from_le_bytes(gf3_remainder_16(&trits)) ^ fnv
}

/// TTX3 档案视图：借用字节流，**按瓦片惰性校验与解码**（流式、低内存）。
#[derive(Debug, Clone)]
pub struct Ttx3Archive<'a> {
    bytes: &'a [u8],
    levels: Vec<ArchiveLevel>,
}

#[derive(Debug, Clone, Copy)]
struct ArchiveLevel {
    width: usize,
    height: usize,
    policy: LevelPolicy,
    dir_offset: u64,
    #[allow(dead_code)]
    tiles: usize,
}

impl<'a> Ttx3Archive<'a> {
    /// 解析头部与级目录（不校验瓦片——那是 [`Ttx3Archive::decode_tile`] /
    /// [`Ttx3Archive::verify`] 的活，逐瓦片惰性）。
    pub fn parse(bytes: &'a [u8]) -> Result<Self, TextureError> {
        if bytes.len() < HEADER_BYTES || bytes[0..4] != MAGIC {
            return Err(TextureError::Ttx3Magic);
        }
        let flags = u16::from_le_bytes([bytes[4], bytes[5]]);
        if flags != 0 {
            return Err(TextureError::Ttx3Flags { flags });
        }
        let level_count = bytes[8] as usize;
        if level_count == 0 || level_count > MAX_LEVELS {
            return Err(TextureError::Ttx3Level {
                level: level_count,
                message: format!("级数必须在 1..={MAX_LEVELS}"),
            });
        }
        let levels_dir_offset = u64::from_le_bytes(bytes[12..20].try_into().unwrap()) as usize;
        if levels_dir_offset != HEADER_BYTES
            || bytes.len() < levels_dir_offset + level_count * LEVEL_ENTRY_BYTES
        {
            return Err(TextureError::Ttx3Truncated {
                needed: levels_dir_offset as u64 + (level_count * LEVEL_ENTRY_BYTES) as u64,
                got: bytes.len() as u64,
            });
        }
        let mut levels = Vec::with_capacity(level_count);
        for index in 0..level_count {
            let at = levels_dir_offset + index * LEVEL_ENTRY_BYTES;
            let width = u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
            let height = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let policy = LevelPolicy::from_byte(bytes[at + 8])?;
            let dir_offset = u64::from_le_bytes(bytes[at + 12..at + 20].try_into().unwrap());
            let (tx, ty) = tile_grid(width, height);
            levels.push(ArchiveLevel {
                width,
                height,
                policy,
                dir_offset,
                tiles: tx * ty,
            });
        }
        Ok(Self { bytes, levels })
    }

    /// 级数。
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// 某级的 (宽, 高, 政策)。
    pub fn level_info(&self, level: usize) -> Result<(usize, usize, LevelPolicy), TextureError> {
        let entry = self.levels.get(level).ok_or(TextureError::Ttx3Level {
            level,
            message: "级不存在".into(),
        })?;
        Ok((entry.width, entry.height, entry.policy))
    }

    /// 某瓦片的目录条目：`(data_offset, copy2_offset, len, gf3_crc16)`。
    /// 流式工具（只读指定瓦片的字节区间）用它定位。
    pub fn tile_entry(
        &self,
        level: usize,
        tx: usize,
        ty: usize,
    ) -> Result<(u64, u64, u32, u32), TextureError> {
        let entry = self.levels.get(level).ok_or(TextureError::Ttx3Level {
            level,
            message: "级不存在".into(),
        })?;
        let (tiles_x, tiles_y) = tile_grid(entry.width, entry.height);
        if tx >= tiles_x || ty >= tiles_y {
            return Err(TextureError::Ttx3Tile {
                level,
                tx,
                ty,
                message: format!("瓦片坐标越界（网格 {tiles_x}×{tiles_y}）"),
            });
        }
        let at = entry.dir_offset as usize + (ty * tiles_x + tx) * TILE_ENTRY_BYTES;
        if self.bytes.len() < at + TILE_ENTRY_BYTES {
            return Err(TextureError::Ttx3Truncated {
                needed: (at + TILE_ENTRY_BYTES) as u64,
                got: self.bytes.len() as u64,
            });
        }
        let off1 = u64::from_le_bytes(self.bytes[at..at + 8].try_into().unwrap());
        let off2 = u64::from_le_bytes(self.bytes[at + 8..at + 16].try_into().unwrap());
        let len = u32::from_le_bytes(self.bytes[at + 16..at + 20].try_into().unwrap());
        let crc = u32::from_le_bytes(self.bytes[at + 20..at + 24].try_into().unwrap());
        Ok((off1, off2, len, crc))
    }

    fn slice_at(&self, offset: u64, len: u32) -> Result<&'a [u8], TextureError> {
        let start = offset as usize;
        let end = start + len as usize;
        if self.bytes.len() < end {
            return Err(TextureError::Ttx3Truncated {
                needed: end as u64,
                got: self.bytes.len() as u64,
            });
        }
        Ok(&self.bytes[start..end])
    }

    /// 取一个瓦片的块数据（12 KB× 冗余的原始形态）：校验 GF(3) CRC-16，
    /// Strength 档单副本损坏自动走第二份恢复。
    pub fn tile_blocks(
        &self,
        level: usize,
        tx: usize,
        ty: usize,
    ) -> Result<Vec<TritBlock>, TextureError> {
        let (off1, off2, len, crc) = self.tile_entry(level, tx, ty)?;
        let mut blocks = Vec::with_capacity(TILE_BLOCKS);
        let mut recovered = false;
        for (attempt, offset) in [off1, off2].into_iter().filter(|o| *o != 0).enumerate() {
            let slice = self.slice_at(offset, len)?;
            if tile_crc(slice) == crc {
                for block_index in 0..TILE_BLOCKS {
                    let mut block = [0u8; TRIT_BLOCK_BYTES];
                    block.copy_from_slice(
                        &slice
                            [block_index * TRIT_BLOCK_BYTES..(block_index + 1) * TRIT_BLOCK_BYTES],
                    );
                    blocks.push(block);
                }
                return Ok(blocks);
            }
            recovered = attempt > 0;
        }
        Err(TextureError::Ttx3Tile {
            level,
            tx,
            ty,
            message: if recovered {
                "两份冗余副本都损坏".into()
            } else {
                "GF(3) CRC-16 校验不符（Capacity 档无冗余可恢复）".into()
            },
        })
    }

    /// 流式解码一个瓦片为 RGBA（320 KB）。
    pub fn decode_tile(
        &self,
        level: usize,
        tx: usize,
        ty: usize,
    ) -> Result<TileRgba, TextureError> {
        let (width, _, _) = self.level_info(level)?;
        let blocks = self.tile_blocks(level, tx, ty)?;
        let texture = TritTexture {
            width: TILE_WIDTH,
            height: TILE_HEIGHT,
            blocks,
        };
        let mut out = vec![0u8; TILE_TEXELS * 4];
        decode_rgba_into(&texture, &mut out)?;
        let _ = width;
        Ok(out)
    }

    /// 全量校验：逐瓦片 GF(3) CRC-16，返回瓦片总数；任何瓦片损坏都是结构化错误。
    pub fn verify(&self) -> Result<usize, TextureError> {
        let mut total = 0usize;
        for level in 0..self.levels.len() {
            let (width, height, _) = self.level_info(level)?;
            let (tiles_x, tiles_y) = tile_grid(width, height);
            for ty in 0..tiles_y {
                for tx in 0..tiles_x {
                    self.tile_blocks(level, tx, ty)?;
                    total += 1;
                }
            }
        }
        Ok(total)
    }
}
