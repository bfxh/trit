//! **TTX**（TritText eXchange）——本仓自有的三进制纹理容器格式。
//!
//! ⚠ **本模块是 v1（4×10 块）的读器，格式常量已冻结。**
//!
//! v2 把块改了 8×8 / 17 字节（见 crate 根文档「块形状为什么从 4×10 改成 8×8」），
//! 因此 v1 的 `to_ttx` / `from_ttx` **只在当前块布局恰好是 4×10 时才成立**；
//! 一旦 `BLOCK_*` 常量离开 4×10，两个函数都会返回 [`TextureError::TtxV1LayoutMismatch`]
//! 而不是用新布局去误读旧字节。v2 的纹理请走 `ttx2`（kind=TEX）。
//!
//! 布局（全部小端，**v1 冻结**）：
//!
//! ```text
//! 偏移  大小  内容
//! 0     4    magic "TTX1"
//! 4     4    width  (u32，4 的倍数)
//! 8     4    height (u32，10 的倍数)
//! 12    4    blocks (u32，块数 = (width/4)·(height/10))
//! 16    N    块载荷：每块 12 字节（4 字节 RGB565 端点对 + 8 字节基 3 索引）
//! 16+N  1    GF(3) CRC-3：对全部块的 40·blocks 个**索引 trit** 计算
//! ```
//!
//! 与二进制容器（PNG 等）的差异：校验和不是比特 CRC，而是 **GF(3) 上的
//! CRC-3**——对三进制内容按三进制运算，`x³+2x+1` 本原，单符号错全检。
//! 头部之后的每个字节都属于格式；加载时逐段校验，不一致即结构化报错。

use trit_core::value::Ternary;

use crate::{
    BLOCK_HEIGHT, BLOCK_WIDTH, TRIT_BLOCK_BYTES, TRITS_PER_BLOCK, TextureError, TritTexture,
};

/// 魔数：`TTX1`（版本随首字节递进）。
pub const MAGIC: [u8; 4] = *b"TTX1";
/// 头部字节数（magic + width + height + blocks）。
pub const HEADER_BYTES: usize = 16;
/// 校验尾字节数（3 trit 打进 1 字节：3³ = 27 ≤ 256）。
pub const CHECKSUM_BYTES: usize = 1;

/// v1 冻结的块形状与块字节数——与 crate 当前的 `BLOCK_*` 常量分开记录。
pub const V1_BLOCK_WIDTH: usize = 4;
/// v1 冻结的块高。
pub const V1_BLOCK_HEIGHT: usize = 10;
/// v1 冻结的块字节数（4 端点 + 8 载荷）。
pub const V1_BLOCK_BYTES: usize = 12;
/// v1 冻结的每块 trit 数。
pub const V1_TRITS_PER_BLOCK: usize = 40;

/// 当前 crate 的块布局是否仍是 v1（4×10 / 12 字节 / 40 trit）。
const fn is_v1_layout() -> bool {
    BLOCK_WIDTH == V1_BLOCK_WIDTH
        && BLOCK_HEIGHT == V1_BLOCK_HEIGHT
        && TRIT_BLOCK_BYTES == V1_BLOCK_BYTES
        && TRITS_PER_BLOCK == V1_TRITS_PER_BLOCK
}

/// GF(3) 生成多项式 `x³ + 2x + 1`（本原），系数高次在前——与 trit-codec 的
/// checksum 同多项式；此处就地实现（实验层互不依赖）。
const GEN: [i32; 4] = [1, 0, 2, 1];

pub(crate) fn gf3_remainder(trits: &[Ternary]) -> [u8; CHECKSUM_BYTES] {
    let digit = |t: Ternary| match t {
        Ternary::False => 0i32,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    };
    let mut buf: Vec<i32> = trits.iter().map(|t| digit(*t)).collect();
    buf.extend_from_slice(&[0; 3]);
    for i in 0..=buf.len() - GEN.len() {
        let coeff = buf[i];
        if coeff != 0 {
            for (j, &g) in GEN.iter().enumerate() {
                buf[i + j] = (buf[i + j] - coeff * g).rem_euclid(3);
            }
        }
    }
    let r = &buf[buf.len() - 3..];
    [(r[0] * 9 + r[1] * 3 + r[2]) as u8]
}

/// 取一个 **v1 块**（12 字节）的 40 个索引 trit。
///
/// 注意：v2 的 `TritBlock` 是 17 字节、64 trit，本函数**不接受**它——v1 与 v2 的
/// 块布局不同源，混用会解出错误内容。v2 的索引抽取在 `ttx2::texture_trits`。
pub(crate) fn block_trits(block: &[u8; V1_BLOCK_BYTES]) -> [Ternary; V1_TRITS_PER_BLOCK] {
    let mut out = [Ternary::False; V1_TRITS_PER_BLOCK];
    for (index, slot) in out.iter_mut().enumerate() {
        // 只取每块的索引 trit 载荷（后 8 字节 = 40 trit，基 3 打包）。
        let byte = block[4 + index / 5];
        let digit = (byte as u32 / 3u32.pow((index % 5) as u32)) % 3;
        *slot = match digit {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        };
    }
    out
}

/// **v2 块**（17 字节 / 64 trit）的索引 trit 抽取。
///
/// 与 `block_trits`（v1，12 字节 / 40 trit）分开：两种块布局不同源，
/// 用同一个函数会在改块形状时静默解出错误内容。
pub(crate) fn block_trits_v2(block: &[u8; TRIT_BLOCK_BYTES]) -> [Ternary; TRITS_PER_BLOCK] {
    let mut out = [Ternary::False; TRITS_PER_BLOCK];
    for (index, slot) in out.iter_mut().enumerate() {
        // 只取索引 trit 载荷（后 TRIT_PAYLOAD_BYTES 字节，基 3 打包，低位在前）。
        let byte = block[4 + index / 5];
        let digit = (byte as u32 / 3u32.pow((index % 5) as u32)) % 3;
        *slot = match digit {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        };
    }
    out
}

fn checksum_for(trits: &[Ternary]) -> u8 {
    gf3_remainder(trits)[0]
}

/// 把纹理序列化成 TTX 容器。
///
/// 当前块布局不是 v1（4×10 / 12 字节 / 40 trit）时返回
/// [`TextureError::TtxV1LayoutMismatch`]——用 8×8 的块去写 "TTX1" 会让旧读器
/// 静默解出错内容，按「绝不猜」的纪律必须显式拒绝。v2 纹理请用
/// [`crate::ttx2::to_ttx2`]。
pub fn to_ttx(texture: &TritTexture) -> Result<Vec<u8>, TextureError> {
    assert_v1_layout()?;
    let mut out =
        Vec::with_capacity(HEADER_BYTES + texture.blocks.len() * V1_BLOCK_BYTES + CHECKSUM_BYTES);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(texture.width as u32).to_le_bytes());
    out.extend_from_slice(&(texture.height as u32).to_le_bytes());
    out.extend_from_slice(&(texture.blocks.len() as u32).to_le_bytes());
    for block in &texture.blocks {
        out.extend_from_slice(block);
    }
    out.push(checksum_for(&block_trits_v2(&texture.blocks[0]))); // guard 已保证不可达
    Ok(out)
}

/// 布局守卫：v1 读写器只对 4×10 块成立。
///
/// 当前块布局已从 4×10 改成 8×8，**该守卫恒为不满足**：TTX1 不再能表示 crate 的
/// `TritTexture`（块字节数、块内 trit 数都变了）。保留本函数是为了让
/// `v1_layout_is_frozen_and_guarded` 测试钉住这个事实，而不是让它悄悄漂移。
fn assert_v1_layout() -> Result<(), TextureError> {
    if is_v1_layout() {
        Ok(())
    } else {
        Err(TextureError::TtxV1LayoutMismatch {
            width: BLOCK_WIDTH,
            height: BLOCK_HEIGHT,
            bytes: TRIT_BLOCK_BYTES,
        })
    }
}

/// 从 TTX 容器加载纹理：魔数、尺寸对齐、块数与载荷长度、GF(3) 校验和
/// 逐段验证——任何不一致都是结构化错误，绝不猜。
///
/// 与 [`to_ttx`] 同理：当前块布局不是 v1 时显式拒绝。
pub fn from_ttx(bytes: &[u8]) -> Result<TritTexture, TextureError> {
    assert_v1_layout()?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES {
        return Err(TextureError::TtxTruncated {
            actual: bytes.len(),
        });
    }
    if bytes[..4] != MAGIC {
        return Err(TextureError::TtxMagic(bytes[..4].try_into().unwrap()));
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    let height = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let blocks = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if width == 0 || !width.is_multiple_of(V1_BLOCK_WIDTH) {
        return Err(TextureError::Shape { width, height });
    }
    if height == 0 || !height.is_multiple_of(V1_BLOCK_HEIGHT) {
        return Err(TextureError::Shape { width, height });
    }
    let expected_blocks = (width / V1_BLOCK_WIDTH) * (height / V1_BLOCK_HEIGHT);
    if blocks != expected_blocks {
        return Err(TextureError::TtxBlockCount {
            declared: blocks as u32,
            expected: expected_blocks,
        });
    }
    let payload_len = blocks * V1_BLOCK_BYTES;
    if bytes.len() != HEADER_BYTES + payload_len + CHECKSUM_BYTES {
        return Err(TextureError::TtxTruncated {
            actual: bytes.len(),
        });
    }
    let mut block_list = Vec::with_capacity(blocks);
    let mut trits = Vec::with_capacity(blocks * V1_TRITS_PER_BLOCK);
    for index in 0..blocks {
        let start = HEADER_BYTES + index * V1_BLOCK_BYTES;
        let mut raw = [0u8; V1_BLOCK_BYTES];
        raw.copy_from_slice(&bytes[start..start + V1_BLOCK_BYTES]);
        trits.extend_from_slice(&block_trits(&raw));
        let mut block = [0u8; TRIT_BLOCK_BYTES];
        let n = V1_BLOCK_BYTES.min(TRIT_BLOCK_BYTES);
        block[..n].copy_from_slice(&raw[..n]);
        block_list.push(block);
    }
    let stored = bytes[HEADER_BYTES + payload_len];
    let expected = checksum_for(&trits);
    if stored != expected {
        return Err(TextureError::TtxChecksum { stored, expected });
    }
    Ok(TritTexture {
        width,
        height,
        blocks: block_list,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_layout_is_frozen_and_guarded() {
        // v2 把块改成 8×8 / 17 字节后，TTX1（4×10 / 12 字节）已无法表示 crate 的
        // TritTexture。读写器必须**显式拒绝**，而不是用新布局去误读旧字节。
        assert!(
            assert_v1_layout().is_err(),
            "当前块布局已不是 v1，守卫必须报错"
        );
        assert!(matches!(
            assert_v1_layout().unwrap_err(),
            TextureError::TtxV1LayoutMismatch { .. }
        ));
        // 冻结常量本身不得随 crate 的 BLOCK_* 漂移。
        assert_eq!(V1_BLOCK_WIDTH, 4);
        assert_eq!(V1_BLOCK_HEIGHT, 10);
        assert_eq!(V1_BLOCK_BYTES, 12);
        assert_eq!(V1_TRITS_PER_BLOCK, 40);
    }

    #[test]
    fn v1_reader_rejects_instead_of_misreading() {
        // 哪怕形状合法的 TTX1 头，在当前布局下也必须得到布局错误，
        // 而不是被 8×8 的解析逻辑解出错误内容。
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&10u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; V1_BLOCK_BYTES]);
        bytes.push(0);
        assert!(matches!(
            from_ttx(&bytes),
            Err(TextureError::TtxV1LayoutMismatch { .. })
        ));
        // 写入侧同样拒绝。
        let texture = TritTexture {
            width: BLOCK_WIDTH,
            height: BLOCK_HEIGHT,
            blocks: vec![[0u8; TRIT_BLOCK_BYTES]; 1],
        };
        assert!(matches!(
            to_ttx(&texture),
            Err(TextureError::TtxV1LayoutMismatch { .. })
        ));
    }
}
