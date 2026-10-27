//! **TTX**（TritText eXchange）——本仓自有的三进制纹理容器格式。
//!
//! 布局（全部小端）：
//!
//! ```text
//! 偏移  大小  内容
//! 0     4    magic "TTX1"
//! 4     4    width  (u32，BLOCK_WIDTH 的倍数)
//! 8     4    height (u32，BLOCK_HEIGHT 的倍数)
//! 12    4    blocks (u32，块数 = (width/4)·(height/8))
//! 16    N    块载荷：每块 TRIT_BLOCK_BYTES(11) 字节
//! 16+N  1    GF(3) CRC-3：对全部块的 32·blocks 个**索引 trit** 计算
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

/// TTX1 的 GF(3) 生成系数（**低次在前**，本实现的消费顺序）：实际除数是
/// `x³ + 2x² + 1`——`x³+2x+1` 的互反多项式，同次数、常数项非零，单 trit 错
/// 全检的保证不变。**TTX1 行为冻结**（兼容优先），不回头改成高次序。
const GEN: [i32; 4] = [1, 0, 2, 1];

/// TTX2 校验尾的 GF(3) 生成系数（**低次在前**）：**g = x¹⁶ + x⁷ + 2**，本原
/// （x 模 g 的阶 = 3¹⁶−1 = 43046720，素因子 {2,5,17,41,193} 逐一排除，独立
/// Python 验算）。16 trit 余数 ≈ 25.4 bit 检错强度，≤16 连续 trit 的突发错全检。
pub(crate) const GEN16: [i32; 17] = [2, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 1];

pub(crate) fn gf3_remainder(trits: &[Ternary]) -> [u8; CHECKSUM_BYTES] {
    let r = gf3_rem(trits, &GEN);
    [(r[0] * 9 + r[1] * 3 + r[2]) as u8]
}

/// GF(3) CRC-16（TTX2 校验尾）：16 个余数 trit 按低位在前打进 4 字节小端
/// （`Σ r_i · 3^i`，3¹⁶ = 43046721 < 2³²）。
pub(crate) fn gf3_remainder_16(trits: &[Ternary]) -> [u8; 4] {
    let r = gf3_rem(trits, &GEN16);
    let mut value = 0u32;
    for (i, digit) in r.iter().enumerate() {
        value += *digit as u32 * 3u32.pow(i as u32);
    }
    value.to_le_bytes()
}

/// GF(3) 多项式除法余数：`poly` 系数**低次在前**且常数项非零，余数次数 < deg。
fn gf3_rem(trits: &[Ternary], poly: &[i32]) -> Vec<i32> {
    let degree = poly.len() - 1;
    let digit = |t: Ternary| match t {
        Ternary::False => 0i32,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    };
    let mut buf: Vec<i32> = trits.iter().map(|t| digit(*t)).collect();
    buf.extend_from_slice(&vec![0; degree]);
    for i in 0..=buf.len() - poly.len() {
        let coeff = buf[i];
        if coeff != 0 {
            for (j, &g) in poly.iter().enumerate() {
                buf[i + j] = (buf[i + j] - coeff * g).rem_euclid(3);
            }
        }
    }
    buf[buf.len() - degree..].to_vec()
}

pub(crate) fn block_trits(block: &[u8; TRIT_BLOCK_BYTES]) -> [Ternary; TRITS_PER_BLOCK] {
    let mut out = [Ternary::False; TRITS_PER_BLOCK];
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

fn checksum(texture: &TritTexture) -> u8 {
    let mut trits = Vec::with_capacity(texture.blocks.len() * TRITS_PER_BLOCK);
    for block in &texture.blocks {
        trits.extend_from_slice(&block_trits(block));
    }
    gf3_remainder(&trits)[0]
}

/// 把纹理序列化成 TTX 容器。
pub fn to_ttx(texture: &TritTexture) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(HEADER_BYTES + texture.blocks.len() * TRIT_BLOCK_BYTES + CHECKSUM_BYTES);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&(texture.width as u32).to_le_bytes());
    out.extend_from_slice(&(texture.height as u32).to_le_bytes());
    out.extend_from_slice(&(texture.blocks.len() as u32).to_le_bytes());
    for block in &texture.blocks {
        out.extend_from_slice(block);
    }
    out.push(checksum(texture));
    out
}

/// 从 TTX 容器加载纹理：魔数、尺寸对齐、块数与载荷长度、GF(3) 校验和
/// 逐段验证——任何不一致都是结构化错误，绝不猜。
pub fn from_ttx(bytes: &[u8]) -> Result<TritTexture, TextureError> {
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
    if width == 0 || !width.is_multiple_of(BLOCK_WIDTH) {
        return Err(TextureError::Shape { width, height });
    }
    if height == 0 || !height.is_multiple_of(BLOCK_HEIGHT) {
        return Err(TextureError::Shape { width, height });
    }
    let expected_blocks = (width / BLOCK_WIDTH) * (height / BLOCK_HEIGHT);
    if blocks != expected_blocks {
        return Err(TextureError::TtxBlockCount {
            declared: blocks as u32,
            expected: expected_blocks,
        });
    }
    // checked：32 位目标上 usize=u32，恶意大 blocks 会回绕骗过下面的长度对账
    //（64 位上长度校验本来就挡住，这里是同一条不变式的显式写法）。
    let payload_len = blocks
        .checked_mul(TRIT_BLOCK_BYTES)
        .ok_or(TextureError::TtxTruncated {
            actual: bytes.len(),
        })?;
    if bytes.len() != HEADER_BYTES + payload_len + CHECKSUM_BYTES {
        return Err(TextureError::TtxTruncated {
            actual: bytes.len(),
        });
    }
    let mut block_list = Vec::with_capacity(blocks);
    for index in 0..blocks {
        let start = HEADER_BYTES + index * TRIT_BLOCK_BYTES;
        let mut block = [0u8; TRIT_BLOCK_BYTES];
        block.copy_from_slice(&bytes[start..start + TRIT_BLOCK_BYTES]);
        block_list.push(block);
    }
    let texture = TritTexture {
        width,
        height,
        blocks: block_list,
    };
    let stored = bytes[HEADER_BYTES + payload_len];
    if stored != checksum(&texture) {
        return Err(TextureError::TtxChecksum {
            stored,
            expected: checksum(&texture),
        });
    }
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{decode_rgba, encode_rgba};

    fn sample(width: usize, height: usize) -> Vec<u8> {
        (0..width * height * 4).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn round_trips_through_container() {
        let pixels = sample(8, BLOCK_HEIGHT);
        let texture = encode_rgba(&pixels, 8, BLOCK_HEIGHT).unwrap();
        let container = to_ttx(&texture);
        // 布局：16 头 + 2 块×11 + 1 校验（8 宽 → 2 列块）。写成表达式，
        // 块尺寸常量变了这里会跟着走，不会再钉死一个过期数字。
        assert_eq!(
            container.len(),
            HEADER_BYTES + 2 * crate::TRIT_BLOCK_BYTES + CHECKSUM_BYTES
        );
        assert_eq!(&container[..4], &MAGIC);
        let back = from_ttx(&container).unwrap();
        assert_eq!(back, texture);
        assert_eq!(decode_rgba(&back).unwrap(), decode_rgba(&texture).unwrap());
    }

    #[test]
    fn rejects_bad_magic_truncation_and_block_count() {
        let texture = encode_rgba(&sample(4, BLOCK_HEIGHT), 4, BLOCK_HEIGHT).unwrap();
        let mut container = to_ttx(&texture);
        // 魔数错。
        container[0] = b'X';
        assert!(matches!(
            from_ttx(&container),
            Err(TextureError::TtxMagic(_))
        ));
        // 截断。
        let mut truncated = to_ttx(&texture);
        truncated.pop();
        assert!(matches!(
            from_ttx(&truncated),
            Err(TextureError::TtxTruncated { .. })
        ));
        // 块数声明与尺寸不符。
        let mut bad_count = to_ttx(&texture);
        bad_count[12] = 9;
        assert!(matches!(
            from_ttx(&bad_count),
            Err(TextureError::TtxBlockCount {
                declared: 9,
                expected: 1
            })
        ));
    }

    #[test]
    fn payload_corruption_is_caught_by_gf3_checksum() {
        // 翻转载荷里的一个 trit（字节 16 = 首块索引 trit 打包首字节）：
        // GF(3) CRC-3 对单符号错全检。
        let texture = encode_rgba(&sample(4, BLOCK_HEIGHT), 4, BLOCK_HEIGHT).unwrap();
        let mut container = to_ttx(&texture);
        container[HEADER_BYTES + 4] ^= 0x1;
        assert!(matches!(
            from_ttx(&container),
            Err(TextureError::TtxChecksum { .. })
        ));
    }

    #[test]
    fn multi_block_checksum_covers_every_block() {
        // 8×16 = 4 块：把最后一块载荷打翻，校验和必须仍然抓到——
        // 防止"只校验了第一块"的假实现。
        let texture = encode_rgba(&sample(8, 16), 8, 16).unwrap();
        assert_eq!(texture.blocks.len(), 4);
        let mut container = to_ttx(&texture);
        let last_block_payload = HEADER_BYTES + 3 * TRIT_BLOCK_BYTES + 4;
        container[last_block_payload] ^= 0x1;
        assert!(matches!(
            from_ttx(&container),
            Err(TextureError::TtxChecksum { .. })
        ));
    }

    #[test]
    fn absurd_block_declarations_are_rejected_without_allocation() {
        // 块数声明落在 32 位回绕区（blocks×11 > u32::MAX）：必须报 TtxTruncated，
        // 绝不做按声明数的分配（payload_len 用 checked 乘法兜底）。
        // 64 位 CI 上这条与「长度不符」同错误码；32 位回绕路径本身无法在 64 位
        // 触发——不变式靠 checked 算术显式成立，故无对应变异锚点（锚点在 64 位
        // 上必然存活，照实记）。
        let texture = encode_rgba(&sample(4, BLOCK_HEIGHT), 4, BLOCK_HEIGHT).unwrap();
        let mut container = to_ttx(&texture);
        let width = 800_000_000u32; // 4 的倍数；expected = 2e8 × 2 = 4e8 块
        let height = 16u32; // 必须是 BLOCK_HEIGHT 的倍数，否则先撞 Shape 而非回绕路径
        let blocks = 400_000_000u32; // blocks × 11 = 4.4e9 > u32::MAX
        container[4..8].copy_from_slice(&width.to_le_bytes());
        container[8..12].copy_from_slice(&height.to_le_bytes());
        container[12..16].copy_from_slice(&blocks.to_le_bytes());
        assert!(matches!(
            from_ttx(&container),
            Err(TextureError::TtxTruncated { .. })
        ));
    }
}
