//! 三值内存布局互转：packed bytes ↔ dual-rail bitplanes。
//!
//! 三值数据在内存里有两种主流表示：
//!
//! - **packed**（[`pack`](crate::pack)）：5 trits/byte，紧凑存储，密度 1.6 bit/trit；
//! - **dual-rail**（trit-core 的 `TriVec`）：两根 bit plane（ones, unknowns），
//!   `(0,0)=False, (1,0)=True, (0,1)=Unknown`，适合位并行运算。
//!
//! 本模块在两种布局之间无损互转，是"存储用 packed、计算用 dual-rail"的桥。
//!
//! 映射：`False=0, Unknown=1, True=2`，与 [`pack`](crate::pack) 一致。

use crate::pack;
use trit_core::value::Ternary;

/// dual-rail 位平面：`(ones, unknowns)`，每位对应一个 trit。
/// `(0,0)=False, (1,0)=True, (0,1)=Unknown`；禁止 `(1,1)`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitplanes {
    /// ones plane：True 位置为 1。
    pub ones: Vec<u64>,
    /// unknowns plane：Unknown 位置为 1。
    pub unknowns: Vec<u64>,
    /// 有效 trit 数（尾部位掩码）。
    pub len: usize,
}

impl Bitplanes {
    /// 构造空布局。
    pub fn new(len: usize) -> Self {
        let words = len.div_ceil(64);
        Self {
            ones: vec![0u64; words],
            unknowns: vec![0u64; words],
            len,
        }
    }

    /// 读第 i 个 trit（要求 `0 <= i < len`）。
    pub fn get(&self, i: usize) -> Ternary {
        let word = i / 64;
        let bit = i % 64;
        let on = (self.ones[word] >> bit) & 1;
        let unk = (self.unknowns[word] >> bit) & 1;
        match (on, unk) {
            (1, 0) => Ternary::True,
            (0, 1) => Ternary::Unknown,
            (0, 0) => Ternary::False,
            (1, 1) => panic!("非法双导轨状态 (1,1) at {i}"),
            _ => unreachable!("bit is 0 or 1"),
        }
    }

    /// 写第 i 个 trit。
    /// 写第 i 个 trit（要求 `0 <= i < len`）。
    pub fn set(&mut self, i: usize, v: Ternary) {
        let word = i / 64;
        let bit = i % 64;
        let mask = 1u64 << bit;
        match v {
            Ternary::True => {
                self.ones[word] |= mask;
                self.unknowns[word] &= !mask;
            }
            Ternary::Unknown => {
                self.ones[word] &= !mask;
                self.unknowns[word] |= mask;
            }
            Ternary::False => {
                self.ones[word] &= !mask;
                self.unknowns[word] &= !mask;
            }
        }
    }
}

/// packed bytes → dual-rail bitplanes。
pub fn unpack_to_bitplanes(bytes: &[u8], len: usize) -> Result<Bitplanes, crate::CodecError> {
    let trits = pack::unpack(bytes, len)?;
    let mut bp = Bitplanes::new(len);
    for (i, t) in trits.iter().enumerate() {
        bp.set(i, *t);
    }
    Ok(bp)
}

/// dual-rail bitplanes → packed bytes。
pub fn bitplanes_to_pack(bp: &Bitplanes) -> Vec<u8> {
    let trits: Vec<Ternary> = (0..bp.len).map(|i| bp.get(i)).collect();
    pack::pack(&trits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_round_trips() {
        let bp = Bitplanes::new(0);
        let bytes = bitplanes_to_pack(&bp);
        assert!(bytes.is_empty());
        let back = unpack_to_bitplanes(&bytes, 0).unwrap();
        assert_eq!(back.len, 0);
    }

    #[test]
    fn small_round_trip() {
        let values = [
            Ternary::True,
            Ternary::False,
            Ternary::Unknown,
            Ternary::True,
            Ternary::Unknown,
        ];
        let bytes = pack::pack(&values);
        let bp = unpack_to_bitplanes(&bytes, values.len()).unwrap();
        let bytes2 = bitplanes_to_pack(&bp);
        assert_eq!(bytes, bytes2);
        // 逐位读回。
        for (i, v) in values.iter().enumerate() {
            assert_eq!(bp.get(i), *v, "pos {i}");
        }
    }

    #[test]
    fn crosses_64_bit_boundary() {
        // 100 个 trit，跨两个 word。
        let mut values = Vec::new();
        for i in 0..100 {
            values.push(match i % 3 {
                0 => Ternary::False,
                1 => Ternary::Unknown,
                _ => Ternary::True,
            });
        }
        let bytes = pack::pack(&values);
        let bp = unpack_to_bitplanes(&bytes, values.len()).unwrap();
        let bytes2 = bitplanes_to_pack(&bp);
        assert_eq!(bytes, bytes2);
        for (i, v) in values.iter().enumerate() {
            assert_eq!(bp.get(i), *v, "pos {i}");
        }
    }

    #[test]
    fn set_then_get_round_trips() {
        let mut bp = Bitplanes::new(10);
        bp.set(0, Ternary::True);
        bp.set(1, Ternary::Unknown);
        bp.set(2, Ternary::False);
        assert_eq!(bp.get(0), Ternary::True);
        assert_eq!(bp.get(1), Ternary::Unknown);
        assert_eq!(bp.get(2), Ternary::False);
        assert_eq!(bp.get(3), Ternary::False); // 默认 False
    }
}
