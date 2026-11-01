//! 三进制块纹理压缩实验层（`publish = false`，不属于稳定 API）。
//!
//! 格式 **TT3**：**8×8 = 64 纹素**的块，块头是 RGB565 端点对（4 字节），
//! 每纹素 **1 个 trit** 索引 3 级调色板 `{e0, mid, e1}`；64 个 trit 按
//! 5 trit/字节打包成 13 字节 ⇒ 每块 **17 字节 = 2.125 bpp**。
//!
//! **TTX3**（万级大纹理档案）：在 TT3 之上做瓦片化 + 层级混搭 + 流式瓦片解码——
//! 基面 10240×10240（1.05 亿纹素）、细层容量档 / 粗层强度档（双写冗余）、
//! 逐瓦片 GF(3) CRC-16、u64 偏移，见 [`ttx3`] 与 docs/ATLAS.md。
//!
//! 对比基线（同一块结构、同一 3 级调色板，索引改 2 bit/纹素）：
//! 64×2 bit = 16 字节 ⇒ 每块 20 字节 = 2.5 bpp。**等调色板下 trit 载荷
//! 13 字节 < 二进制 16 字节（省 18.75% 载荷 / 15% 码率）**——可复算，
//! `cargo run -p trit-texture` 打表。
//!
//! 四类边界如实记账（见 docs/TERNARY-BOUNDARIES.md）：
//! - **A 原生红利**：3 级调色板索引 1 trit（log₂3 ≈ 1.585 bit）装得下，2 bit 装不满
//!   ——64 纹素取整后 13 字节 vs 16 字节（省 18.75% 载荷）。
//! - **B 模拟税**：64 纹素占 65 槽，打包粒度税 **1.6%**（v1 的 4×10 把这项清零，
//!   却换来 25% 的 mip 补边，得不偿失）；端点被迫走 RGB565 仍是量化税。
//! - **C 生态路径依赖**：RGB565 端点对抄自 BC1 的生态格式，不是三进制选择；
//!   无硬件采样器是自有格式的宿命。
//! - **D 基数无关**：最近调色板色搜索、端点优化（最小二乘精修）与进制无关。
//!
//! ## 块形状为什么从 4×10 改成 8×8
//!
//! 4×10 的唯一卖点是 40 恰被 5 整除、载荷零浪费。但 10 不是 2 的幂，而 mip 每级
//! 高度减半仍须 %10，于是正常 2 的幂纹理必须补到 `10·2^(n-1)` 的倍数——**恒定
//! 25% 填充**，名义 2.40 bpp 实际 3.00 bpp。8×8 付 1.6% 打包税，换来全程合法
//! 的 mip 链与 2.125 bpp。**「零槽位浪费」是局部最优，mip 合法性才是全局最优。**

#![forbid(unsafe_code)]

use trit_core::value::Ternary;

pub mod convert;
mod endpoints;
pub mod error;
pub mod gguf;
pub mod mip;
#[cfg(feature = "png")]
pub mod pngio;
pub mod preview;
pub mod raster;
pub mod sample;
pub mod ttx;
pub mod ttx2;
pub mod ttx3;

pub use error::TextureError;
pub use ttx3::{
    LargeTexture, LevelPolicy, TILE_BLOCKS, TILE_DATA_BYTES, TILE_HEIGHT, TILE_TEXELS, TILE_WIDTH,
    TileRgba, Ttx3Archive, level_storage_bytes, suggest_chain,
};

use endpoints::{block_endpoints, nearest_index, palette_from_endpoints, refine_endpoints};

/// 块宽：**8×8 方块**（v2 起）。
///
/// > 教训（v1 的 4×10）：40 纹素恰被 5 整除 ⇒ 载荷 8 字节零浪费，看着很美。
/// > 但 10 不是 2 的幂，而每个 mip 级高度都要 %10，减半后很快不合法：正常 2 的幂
/// > 纹理（256/512/1024/2048）**没有一个**是 10 的倍数，必须补到 320/640/1280/2560
/// > ——**恒定 25% 高度填充**。名义 2.40 bpp 实际 3.00 bpp，相对 BC1 从「省 40%」
/// > 掉到「省 25%」。
/// >
/// > 取舍：2^n 永远不被 5 整除，**「零槽位浪费」与「mip 友好」不可兼得**。选后者。
pub const BLOCK_WIDTH: usize = 8;
/// 块高：8×8 方块，与宽相同 ⇒ 任意 2 的幂尺寸全程可 mip 到 1×1。
pub const BLOCK_HEIGHT: usize = 8;
/// 每块纹素数。
pub const BLOCK_PIXELS: usize = BLOCK_WIDTH * BLOCK_HEIGHT;
/// 每块 trit 索引数。
pub const TRITS_PER_BLOCK: usize = BLOCK_PIXELS;
/// trit 载荷字节数：ceil(64/5) = 13（65 槽用 64，打包税 **1.6%**；5 trit/字节 = 3^5 = 243 ≤ 256）。
///
/// 用 1.6% 的打包税换掉 v1 的 25% mip 补边——净赚 23.4 个百分点。
pub const TRIT_PAYLOAD_BYTES: usize = TRITS_PER_BLOCK.div_ceil(5);
/// 块总字节数：4 字节端点 + 13 字节 trit 载荷 = 17 字节 ⇒ **2.125 bpp**。
pub const TRIT_BLOCK_BYTES: usize = 4 + TRIT_PAYLOAD_BYTES;
/// 二进制基线载荷字节数：64 纹素 × 2 bit = 128 bit = 16 字节。
pub const BINARY_PAYLOAD_BYTES: usize = BLOCK_PIXELS * 2 / 8;
/// 二进制基线块总字节数：4 + 16 = 20 字节 ⇒ 2.5 bpp。
pub const BINARY_BLOCK_BYTES: usize = 4 + BINARY_PAYLOAD_BYTES;
/// 纹素数上限（解析/分配之前拒绝）。
pub const MAX_TEXTURE_PIXELS: usize = 1 << 22;

/// RGB565 打包：R 高 5 位、G 中 6 位、B 低 5 位。
fn pack_rgb565(r: u8, g: u8, b: u8) -> u16 {
    (u16::from(r >> 3) << 11) | (u16::from(g >> 2) << 5) | u16::from(b >> 3)
}

/// RGB565 展开（左对齐补位，`(x << 3) | (x >> 2)` 风格，让 0/31 满量程）。
fn unpack_rgb565(value: u16) -> (u8, u8, u8) {
    let r5 = (value >> 11) as u8 & 0x1f;
    let g6 = (value >> 5) as u8 & 0x3f;
    let b5 = value as u8 & 0x1f;
    (
        (r5 << 3) | (r5 >> 2),
        (g6 << 2) | (g6 >> 4),
        (b5 << 3) | (b5 >> 2),
    )
}

/// 5 trit/字节打包（低位在前）：`Σ digit_i · 3^i`，3^5 = 243 ≤ 256。
fn trit_digit(value: Ternary) -> u32 {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

fn pack_trits(values: &[Ternary]) -> [u8; TRIT_PAYLOAD_BYTES] {
    let mut out = [0u8; TRIT_PAYLOAD_BYTES];
    for (index, value) in values.iter().enumerate() {
        let byte = index / 5;
        let slot = index % 5;
        out[byte] += (trit_digit(*value) * 3u32.pow(slot as u32)) as u8;
    }
    out
}

pub(crate) fn unpack_trits(bytes: &[u8; TRIT_PAYLOAD_BYTES]) -> [Ternary; TRITS_PER_BLOCK] {
    let mut out = [Ternary::False; TRITS_PER_BLOCK];
    for (index, slot) in out.iter_mut().enumerate() {
        let digit = (bytes[index / 5] as u32 / 3u32.pow((index % 5) as u32)) % 3;
        *slot = match digit {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        };
    }
    out
}

/// 一个 TT3 块：4 字节 RGB565 端点 + 13 字节 trit 载荷 = 17 字节。
pub type TritBlock = [u8; TRIT_BLOCK_BYTES];

/// TT3 编码后的纹理：`blocks[y / 8][x / 8]` 行主序。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TritTexture {
    /// 宽（8 的倍数）。
    pub width: usize,
    /// 高（8 的倍数）。
    pub height: usize,
    /// 块数据。
    pub blocks: Vec<TritBlock>,
}

/// 把 RGBA 像素编码成 TT3。
///
/// 宽高都须是 8 的倍数（8×8 方块）；输入长度必须恰为 `width × height × 4`。
pub fn encode_rgba(
    pixels: &[u8],
    width: usize,
    height: usize,
) -> Result<TritTexture, TextureError> {
    validate(width, height, pixels.len())?;
    let block_cols = width / BLOCK_WIDTH;
    let block_rows = height / BLOCK_HEIGHT;
    let mut blocks = Vec::with_capacity(block_cols * block_rows);
    for block_row in 0..block_rows {
        for block_col in 0..block_cols {
            let origin = (block_col * BLOCK_WIDTH, block_row * BLOCK_HEIGHT);
            let mut texels = [[0u8; 4]; BLOCK_PIXELS];
            for (index, texel) in texels.iter_mut().enumerate() {
                let x = origin.0 + index % BLOCK_WIDTH;
                let y = origin.1 + index / BLOCK_WIDTH;
                let offset = (y * width + x) * 4;
                *texel = [
                    pixels[offset],
                    pixels[offset + 1],
                    pixels[offset + 2],
                    pixels[offset + 3],
                ];
            }
            blocks.push(encode_block(&texels));
        }
    }
    Ok(TritTexture {
        width,
        height,
        blocks,
    })
}

/// 单块编码：min/max 端点 + 最小二乘精修，每纹素最近调色板色的 trit 索引。
pub fn encode_block(texels: &[[u8; 4]; BLOCK_PIXELS]) -> TritBlock {
    let (e0, e1) = block_endpoints(texels);
    encode_block_with_endpoints(texels, e0, e1)
}

/// 同 [`encode_block`]，但端点由调用方给（已做 RGB565 量化）——精修/标定入口。
pub fn encode_block_with_endpoints(
    texels: &[[u8; 4]; BLOCK_PIXELS],
    e0: u16,
    e1: u16,
) -> TritBlock {
    let (e0, e1) = refine_endpoints(texels, e0, e1);
    let palette = palette_from_endpoints(e0, e1);
    let mut indices = [Ternary::False; TRITS_PER_BLOCK];
    for (index, pixel) in texels.iter().enumerate() {
        indices[index] = match nearest_index(&palette, pixel[0], pixel[1], pixel[2]) {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        };
    }
    let mut block = [0u8; TRIT_BLOCK_BYTES];
    block[0..2].copy_from_slice(&e0.to_be_bytes());
    block[2..4].copy_from_slice(&e1.to_be_bytes());
    block[4..].copy_from_slice(&pack_trits(&indices));
    block
}

/// 解码 TT3 回 RGBA（alpha 恒 255）。
pub fn decode_rgba(texture: &TritTexture) -> Result<Vec<u8>, TextureError> {
    let mut out = vec![0u8; texture.width * texture.height * 4];
    decode_rgba_into(texture, &mut out)?;
    Ok(out)
}

/// 解码到**调用方缓冲**：热路径复用同一份输出，免掉整图分配（显存/内存账）。
///
/// `out` 必须恰为 `width × height × 4` 字节；解码器不做隐式清零，逐字节覆写。
pub fn decode_rgba_into(texture: &TritTexture, out: &mut [u8]) -> Result<(), TextureError> {
    let block_cols = texture.width / BLOCK_WIDTH;
    let block_rows = texture.height / BLOCK_HEIGHT;
    let expected = block_cols * block_rows;
    if texture.blocks.len() != expected {
        return Err(TextureError::BlockCount {
            actual: texture.blocks.len(),
            expected,
        });
    }
    if out.len() != texture.width * texture.height * 4 {
        return Err(TextureError::SizeMismatch {
            actual: out.len(),
            expected: texture.width * texture.height * 4,
        });
    }
    for (block_index, block) in texture.blocks.iter().enumerate() {
        let block_col = block_index % block_cols;
        let block_row = block_index / block_cols;
        let e0 = u16::from_be_bytes([block[0], block[1]]);
        let e1 = u16::from_be_bytes([block[2], block[3]]);
        let mut payload = [0u8; TRIT_PAYLOAD_BYTES];
        payload.copy_from_slice(&block[4..]);
        let indices = unpack_trits(&payload);
        let palette = palette_from_endpoints(e0, e1);
        for index in 0..BLOCK_PIXELS {
            let x = block_col * BLOCK_WIDTH + index % BLOCK_WIDTH;
            let y = block_row * BLOCK_HEIGHT + index / BLOCK_WIDTH;
            let offset = (y * texture.width + x) * 4;
            let (r, g, b) = palette[match indices[index] {
                Ternary::False => 0,
                Ternary::Unknown => 1,
                Ternary::True => 2,
            }];
            out[offset] = r;
            out[offset + 1] = g;
            out[offset + 2] = b;
            out[offset + 3] = 255;
        }
    }
    Ok(())
}

/// 二进制基线纹理：同一块结构、同一 3 级调色板，索引 2 bit/纹素。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryTexture {
    /// 宽。
    pub width: usize,
    /// 高。
    pub height: usize,
    /// 块数据（4 字节端点 + 16 字节位载荷 = 20 字节）。
    pub blocks: Vec<[u8; BINARY_BLOCK_BYTES]>,
}

/// 把 RGBA 像素编码成二进制基线（与 TT3 同端点、同调色板，仅索引编码不同）。
pub fn encode_rgba_baseline(
    pixels: &[u8],
    width: usize,
    height: usize,
) -> Result<BinaryTexture, TextureError> {
    validate(width, height, pixels.len())?;
    let block_cols = width / BLOCK_WIDTH;
    let block_rows = height / BLOCK_HEIGHT;
    let mut blocks = Vec::with_capacity(block_cols * block_rows);
    for block_row in 0..block_rows {
        for block_col in 0..block_cols {
            let mut texels = [[0u8; 4]; BLOCK_PIXELS];
            for (index, texel) in texels.iter_mut().enumerate() {
                let x = block_col * BLOCK_WIDTH + index % BLOCK_WIDTH;
                let y = block_row * BLOCK_HEIGHT + index / BLOCK_WIDTH;
                let offset = (y * width + x) * 4;
                *texel = [
                    pixels[offset],
                    pixels[offset + 1],
                    pixels[offset + 2],
                    pixels[offset + 3],
                ];
            }
            blocks.push(encode_block_baseline(&texels));
        }
    }
    Ok(BinaryTexture {
        width,
        height,
        blocks,
    })
}

/// 单块基线编码：索引 2 bit/纹素，LSB 先行。
///
/// 端点走**未精修的 min/max**（与 TT3 精修前的行为一致），这样基线与 TT3 的
/// 对比仍然只差「索引进制」这一个变量；精修带来的增益在 `encode_block` 侧。
pub fn encode_block_baseline(texels: &[[u8; 4]; BLOCK_PIXELS]) -> [u8; BINARY_BLOCK_BYTES] {
    let (e0, e1) = block_endpoints(texels);
    let palette = palette_from_endpoints(e0, e1);
    let mut payload = [0u8; BINARY_PAYLOAD_BYTES];
    for (index, pixel) in texels.iter().enumerate() {
        let palette_index = nearest_index(&palette, pixel[0], pixel[1], pixel[2]) as u64;
        let bit = index * 2;
        payload[bit / 8] |= ((palette_index << (bit % 8)) & 0xff) as u8;
        if bit % 8 > 6 {
            payload[bit / 8 + 1] |= (palette_index >> (8 - bit % 8)) as u8;
        }
    }
    let mut block = [0u8; BINARY_BLOCK_BYTES];
    block[0..2].copy_from_slice(&e0.to_be_bytes());
    block[2..4].copy_from_slice(&e1.to_be_bytes());
    block[4..].copy_from_slice(&payload);
    block
}

/// 解码二进制基线回 RGBA。
pub fn decode_rgba_baseline(texture: &BinaryTexture) -> Result<Vec<u8>, TextureError> {
    let block_cols = texture.width / BLOCK_WIDTH;
    let block_rows = texture.height / BLOCK_HEIGHT;
    let expected = block_cols * block_rows;
    if texture.blocks.len() != expected {
        return Err(TextureError::BlockCount {
            actual: texture.blocks.len(),
            expected,
        });
    }
    let mut out = vec![0u8; texture.width * texture.height * 4];
    for (block_index, block) in texture.blocks.iter().enumerate() {
        let block_col = block_index % block_cols;
        let block_row = block_index / block_cols;
        let e0 = u16::from_be_bytes([block[0], block[1]]);
        let e1 = u16::from_be_bytes([block[2], block[3]]);
        let palette = palette_from_endpoints(e0, e1);
        for index in 0..BLOCK_PIXELS {
            let bit = index * 2;
            let mut palette_index = (block[4 + bit / 8] as u64 >> (bit % 8)) & 0x3;
            if bit % 8 > 6 {
                palette_index |= (block[4 + bit / 8 + 1] as u64) << (8 - bit % 8) & 0x3;
            }
            let x = block_col * BLOCK_WIDTH + index % BLOCK_WIDTH;
            let y = block_row * BLOCK_HEIGHT + index / BLOCK_WIDTH;
            let offset = (y * texture.width + x) * 4;
            // 复审 M1：2-bit 字段可以取 3，而 palette 只有 3 色；越界必须返回错误而不是 panic。
            let Some(&(r, g, b)) = palette.get(palette_index as usize) else {
                return Err(TextureError::InvalidPaletteIndex {
                    block: block_index,
                    index: palette_index as u8,
                });
            };
            out[offset] = r;
            out[offset + 1] = g;
            out[offset + 2] = b;
            out[offset + 3] = 255;
        }
    }
    Ok(out)
}

/// 两块同尺寸 RGBA 缓冲的 PSNR（按 RGB 通道，忽略 alpha）。
/// 完全相同时返回 `None`（MSE = 0，PSNR 无穷）。
pub fn psnr_rgb(a: &[u8], b: &[u8]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let mut total = 0u64;
    let mut count = 0u64;
    for (left, right) in a.iter().zip(b.iter()) {
        let diff = i64::from(*left) - i64::from(*right);
        total += (diff * diff) as u64;
        count += 1;
    }
    // 每像素取 RGB 三通道（跳过每 4 字节里的 alpha）。
    let pixels = count / 4;
    if pixels == 0 {
        return None;
    }
    let mse = (total as f64) / (pixels as f64 * 3.0);
    if mse == 0.0 {
        return None;
    }
    Some(10.0 * (255.0 * 255.0 / mse).log10())
}

fn validate(width: usize, height: usize, len: usize) -> Result<(), TextureError> {
    if width == 0
        || height == 0
        || !width.is_multiple_of(BLOCK_WIDTH)
        || !height.is_multiple_of(BLOCK_HEIGHT)
    {
        return Err(TextureError::Shape { width, height });
    }
    let pixels = width
        .checked_mul(height)
        .ok_or(TextureError::TooLarge { pixels: usize::MAX })?;
    if pixels > MAX_TEXTURE_PIXELS {
        return Err(TextureError::TooLarge { pixels });
    }
    let expected = pixels * 4;
    if len != expected {
        return Err(TextureError::SizeMismatch {
            actual: len,
            expected,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
