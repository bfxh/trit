//! 5 trits / byte 打包。
//!
//! `3^5 = 243 ≤ 256`，所以一个字节正好放 5 个 trit（低位在前），密度 **1.6 bit/trit**。
//! 对比本仓双导轨表示（2 bit/trit）省 20%；对比文本 `0/1/X`（1 byte/trit）省 80%。
//! 代价是每个字节有 13 个不可达值（243..=255），解码时必须拒绝。
//!
//! # 线格式（wire format，稳定契约）
//!
//! 这一节是**格式契约**，改动它等于改格式，必须走新版本号而不是原地调整。
//!
//! 1. **数字映射**：`False = 0`、`Unknown = 1`、`True = 2`。
//!    与 [`Ternary`] 的枚举序号一致，但**不要依赖** `Ternary` 的 serde 表示——那是另一套编码。
//! 2. **低位 trit 优先**：一个字节内 `trit[0]` 是最低有效位，权重 `3^0`；
//!    `trit[4]` 权重 `3^4 = 81`。因此 `byte = Σ trit[i] · 3^i`。
//! 3. **合法字节**：`0..=242`。`243..=255` 是非法码，解码必须返回
//!    [`CodecError::InvalidByte`]，不得折叠成某个逻辑值。
//! 4. **尾部填充**：最后一个字节里未被使用的 trit 槽位**必须是 `False`（0）**。
//!    编码器总是输出这种 canonical 形式；解码器用 [`unpack_canonical`] 可以拒绝非规范输入。
//! 5. **长度规则**：`len` 个 trit 的 canonical 字节数是 `len.div_ceil(5)`。
//!    [`pack`] 输出的字节数恒等于它；[`unpack_canonical`] 要求输入字节数精确等于它。
//! 6. **字节序**：字节之间按序列顺序排列（第 0 个字节在前）。
//!    本格式**不**定义多字节整数的字节序，也不把 Rust 内存布局当格式。
//!
//! # 两套入口
//!
//! - [`pack`] / [`unpack`]：宽松路径，保留给既有调用方。`unpack` 允许 `bytes` 比
//!   `len` 所需的多，多余的尾部 trit 被丢弃，**不检查** padding 是否规范。
//! - [`Packed5`] / [`Packed5Vec`] / [`unpack_canonical`]：严格路径。构造期一次性
//!   校验字节范围与 padding，之后访问不再重复检查。

use trit_core::value::Ternary;

use crate::CodecError;

/// 一个字节能装几个 trit。
pub const TRITS_PER_BYTE: usize = 5;

/// 每字节最多表示的组合数。
pub const COMBINATIONS: u16 = 243;

/// `3^i`，`i ∈ 0..5`。
const POW3: [u16; TRITS_PER_BYTE] = [1, 3, 9, 27, 81];

/// 把 trit 序列打包成字节；最后不足 5 个的低位补 `F`（解码时按长度截断）。
pub fn pack(values: &[Ternary]) -> Vec<u8> {
    values
        .chunks(TRITS_PER_BYTE)
        .map(|chunk| {
            let mut byte = 0u8;
            let mut power = 1u8;
            for value in chunk {
                byte += digit(*value) * power;
                power *= 3;
            }
            byte
        })
        .collect()
}

/// 解码前 `len` 个 trit；`bytes` 里没被用到的尾部 trit 会被丢弃。
///
/// 宽松路径：不校验 padding 是否 canonical，也允许 `bytes` 长于 `len` 所需。
/// 需要严格校验时用 [`unpack_canonical`]。
pub fn unpack(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    if len > bytes.len() * TRITS_PER_BYTE {
        return Err(CodecError::Length {
            expected: len,
            available: bytes.len() * TRITS_PER_BYTE,
        });
    }
    let mut out = Vec::with_capacity(len.min(bytes.len() * TRITS_PER_BYTE));
    for byte in bytes {
        if u16::from(*byte) >= COMBINATIONS {
            return Err(CodecError::InvalidByte(*byte));
        }
        let mut rest = u16::from(*byte);
        for _ in 0..TRITS_PER_BYTE {
            out.push(match rest % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            });
            rest /= 3;
        }
    }
    out.truncate(len);
    Ok(out)
}

/// 严格解码：字节数必须精确等于 `len.div_ceil(5)`，且尾部填充必须是 `False`。
///
/// 这是**唯一**能保证「同一逻辑值只有一种字节表示」的入口。
/// 用宽松的 [`unpack`] 接受非规范输入，会让同一序列出现多种字节串，
/// 进而破坏哈希、签名与相等比较。
pub fn unpack_canonical(bytes: &[u8], len: usize) -> Result<Vec<Ternary>, CodecError> {
    let expected = len.div_ceil(TRITS_PER_BYTE);
    if bytes.len() != expected {
        return Err(CodecError::WrongByteCount {
            expected,
            actual: bytes.len(),
        });
    }
    let mut out = Vec::with_capacity(len);
    for byte in bytes {
        let packed = Packed5::try_new(*byte)?;
        let take = TRITS_PER_BYTE.min(len - out.len());
        for i in 0..take {
            out.push(packed.digit(i));
        }
        // 未使用的槽位必须是 False；否则不是 canonical 编码
        for i in take..TRITS_PER_BYTE {
            if packed.digit(i) != Ternary::False {
                return Err(CodecError::NonCanonicalPadding(*byte));
            }
        }
    }
    Ok(out)
}

/// 同样内容在双导轨表示下要多少字节（2 bit/trit）。
pub fn dual_rail_bytes(len: usize) -> usize {
    len.div_ceil(4)
}

fn digit(value: Ternary) -> u8 {
    match value {
        Ternary::False => 0,
        Ternary::Unknown => 1,
        Ternary::True => 2,
    }
}

/// 一个字节里的 5 个 trit。
///
/// 不变量：内部 `u8` 恒 `< 243`。构造函数是唯一的对外入口，
/// 因此 [`Packed5::digit`] 不需要再判非法码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Packed5(u8);

impl Packed5 {
    /// 一个 `Packed5` 能装几个 trit。
    pub const CAPACITY: usize = TRITS_PER_BYTE;

    /// 外部字节入口：拒绝 `243..=255`。
    pub fn try_new(byte: u8) -> Result<Self, CodecError> {
        if u16::from(byte) >= COMBINATIONS {
            return Err(CodecError::InvalidByte(byte));
        }
        Ok(Self(byte))
    }

    /// 由 5 个 trit 构造（低位在前）。
    pub fn from_digits(digits: [Ternary; TRITS_PER_BYTE]) -> Self {
        let mut code = 0u16;
        for (i, d) in digits.iter().enumerate() {
            code += u16::from(digit(*d)) * POW3[i];
        }
        debug_assert!(code < COMBINATIONS);
        Self(code as u8)
    }

    /// 底层字节。
    pub fn as_byte(self) -> u8 {
        self.0
    }

    /// 取第 `index` 个 trit。`index` 必须 `< 5`。
    pub fn digit(self, index: usize) -> Ternary {
        debug_assert!(index < TRITS_PER_BYTE);
        let weight = POW3[index];
        match (u16::from(self.0) / weight) % 3 {
            0 => Ternary::False,
            1 => Ternary::Unknown,
            _ => Ternary::True,
        }
    }

    /// 展开成 5 个 trit。
    pub fn unpack(self) -> [Ternary; TRITS_PER_BYTE] {
        core::array::from_fn(|i| self.digit(i))
    }
}

/// 变长、canonical 的 5-trit/byte 序列。
///
/// 构造期一次性校验：字节数精确、每个字节 `< 243`、尾部填充为 `False`。
/// 之后 [`Packed5Vec::get`] 不再做任何检查。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed5Vec {
    len: usize,
    bytes: Vec<u8>,
}

impl Packed5Vec {
    /// 由 trit 序列构造；输出恒为 canonical。
    pub fn from_trits(values: &[Ternary]) -> Self {
        Self {
            len: values.len(),
            bytes: pack(values),
        }
    }

    /// 外部字节入口：校验字节数、非法码与 padding。
    pub fn try_from_bytes(bytes: Vec<u8>, len: usize) -> Result<Self, CodecError> {
        // 复用严格解码走一遍校验，避免两套规则各写一遍
        unpack_canonical(&bytes, len)?;
        Ok(Self { len, bytes })
    }

    /// trit 个数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// canonical 字节视图。
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// 取第 `index` 个 trit；越界返回 `None`。
    pub fn get(&self, index: usize) -> Option<Ternary> {
        if index >= self.len {
            return None;
        }
        let byte = *self.bytes.get(index / TRITS_PER_BYTE)?;
        // 构造期已保证 byte < 243
        Some(Packed5(byte).digit(index % TRITS_PER_BYTE))
    }

    /// 展开成 trit 向量。
    pub fn unpack(&self) -> Vec<Ternary> {
        (0..self.len).filter_map(|i| self.get(i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn five_trits_fit_in_one_byte() {
        // TTTTT = 2 + 6 + 18 + 54 + 162 = 242，是最后一个合法字节。
        let all_true = [Ternary::True; 5];
        assert_eq!(pack(&all_true), vec![242]);
        assert_eq!(unpack(&[242], 5).unwrap(), all_true.to_vec());
    }

    #[test]
    fn low_digit_comes_first() {
        // 低位在前：第一个 trit 是 1 的系数。
        assert_eq!(
            pack(&[
                Ternary::False,
                Ternary::False,
                Ternary::False,
                Ternary::False,
                Ternary::True
            ]),
            vec![162]
        );
        assert_eq!(pack(&[Ternary::True]), vec![2]);
    }

    #[test]
    fn round_trip_with_ragged_tail() {
        let values = [
            Ternary::True,
            Ternary::Unknown,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
            Ternary::True,
            Ternary::False,
        ];
        let bytes = pack(&values);
        assert_eq!(bytes.len(), 2);
        assert_eq!(unpack(&bytes, values.len()).unwrap(), values.to_vec());
        // 尾部未使用的 trit 是 F，按容量解码会多出来。
        assert_eq!(unpack(&bytes, 10).unwrap()[7..], [Ternary::False; 3]);
    }

    #[test]
    fn invalid_byte_is_rejected() {
        assert!(matches!(
            unpack(&[243], 5),
            Err(CodecError::InvalidByte(243))
        ));
        assert!(matches!(
            unpack(&[255], 5),
            Err(CodecError::InvalidByte(255))
        ));
    }

    #[test]
    fn length_is_checked() {
        assert!(matches!(unpack(&[0], 6), Err(CodecError::Length { .. })));
    }

    #[test]
    fn density_beats_dual_rail() {
        let len = 100;
        assert_eq!(pack(&vec![Ternary::Unknown; len]).len(), len.div_ceil(5));
        assert_eq!(dual_rail_bytes(len), 25);
        assert!(len.div_ceil(5) < dual_rail_bytes(len));
    }

    // ---- 以下为 canonical / Packed5 / Packed5Vec ----

    #[test]
    fn canonical_round_trip_all_lengths() {
        for len in [0usize, 1, 2, 4, 5, 6, 9, 10, 11, 37, 100] {
            let values: Vec<Ternary> = (0..len)
                .map(|i| match (i * 7 + 2) % 3 {
                    0 => Ternary::False,
                    1 => Ternary::Unknown,
                    _ => Ternary::True,
                })
                .collect();
            let bytes = pack(&values);
            assert_eq!(bytes.len(), len.div_ceil(5), "len {len}");
            assert_eq!(unpack_canonical(&bytes, len).unwrap(), values, "len {len}");
            let p = Packed5Vec::from_trits(&values);
            assert_eq!(p.as_bytes(), bytes.as_slice());
            assert_eq!(p.unpack(), values);
        }
    }

    #[test]
    fn canonical_rejects_non_canonical_padding() {
        // len=1 只需 1 个字节，但第二个槽位放了 True（2）而不是 False
        let bad = Packed5::from_digits([
            Ternary::True,
            Ternary::True,
            Ternary::False,
            Ternary::False,
            Ternary::False,
        ]);
        assert_eq!(
            unpack_canonical(&[bad.as_byte()], 1),
            Err(CodecError::NonCanonicalPadding(bad.as_byte()))
        );
        // 宽松路径仍接受（记录既有行为，不改）
        assert_eq!(unpack(&[bad.as_byte()], 1).unwrap(), vec![Ternary::True]);
    }

    #[test]
    fn canonical_rejects_wrong_byte_count() {
        assert_eq!(
            unpack_canonical(&[0u8; 3], 5),
            Err(CodecError::WrongByteCount {
                expected: 1,
                actual: 3
            })
        );
        assert_eq!(
            unpack_canonical(&[0u8; 1], 6),
            Err(CodecError::WrongByteCount {
                expected: 2,
                actual: 1
            })
        );
    }

    #[test]
    fn canonical_rejects_invalid_byte() {
        assert_eq!(
            unpack_canonical(&[243], 5),
            Err(CodecError::InvalidByte(243))
        );
    }

    #[test]
    fn packed5_all_243_roundtrip() {
        for code in 0u16..COMBINATIONS {
            let p = Packed5::try_new(code as u8).unwrap();
            assert_eq!(Packed5::from_digits(p.unpack()), p, "code {code}");
        }
        for code in 243u8..=255 {
            assert!(matches!(
                Packed5::try_new(code),
                Err(CodecError::InvalidByte(_))
            ));
        }
    }

    #[test]
    fn packed5vec_matches_loose_path() {
        // 差分：严格路径与既有 pack/unpack 在同一输入上必须一致
        let mut state = 0x1234_5678u32;
        for _ in 0..200 {
            let len = (state % 41) as usize;
            let values: Vec<Ternary> = (0..len)
                .map(|_| {
                    state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    match (state >> 16) % 3 {
                        0 => Ternary::False,
                        1 => Ternary::Unknown,
                        _ => Ternary::True,
                    }
                })
                .collect();
            let bytes = pack(&values);
            assert_eq!(unpack(&bytes, len).unwrap(), values);
            assert_eq!(unpack_canonical(&bytes, len).unwrap(), values);
            assert_eq!(Packed5Vec::from_trits(&values).unpack(), values);
        }
    }

    #[test]
    fn packed5vec_get_is_bounds_checked() {
        let p = Packed5Vec::from_trits(&[Ternary::True, Ternary::Unknown]);
        assert_eq!(p.get(0), Some(Ternary::True));
        assert_eq!(p.get(1), Some(Ternary::Unknown));
        assert_eq!(p.get(2), None);
        assert_eq!(Packed5Vec::from_trits(&[]).len(), 0);
    }
}
