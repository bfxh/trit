//! 位并行三值运算：**一条 u64 同时处理 32 个 trit**（双导轨编码）。
//!
//! 编码：每个 trit 用 2 位表示——
//! - `+1` = ones 位为 1，zeros 位为 0
//! - ` 0` = ones 位为 0，zeros 位为 0
//! - `-1` = ones 位为 0，zeros 位为 1
//!
//! `(1,1)` 组合非法。一条 `u32` 存 32 个 ones 位 + 一条 `u32` 存 32 个 zeros 位
//! = 32 个 trit 打包进 2 条 `u32`。
//!
//! K3 运算在打包域上逐位执行——**一条 CPU 指令处理 32 个 trit**（SIMD-like）。
//! 32 万 trit 的 AND 折叠 = 1 万条 u32 AND 指令，不是 32 万次循环。
//!
//! 来自 [bfxh/trit](https://github.com/bfxh/trit) 的双导轨位平面技术。

use crate::trit::Trit;

/// 32 个 trit 的位并行容器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriWord {
    /// 32 个 ones 位。
    pub ones: u32,
    /// 32 个 zeros 位。
    pub zeros: u32,
}

impl TriWord {
    pub const FULL: u32 = u32::MAX;
    pub const EMPTY: u32 = 0;

    /// 全 +1。
    pub const ALL_HARDENED: TriWord = TriWord {
        ones: u32::MAX,
        zeros: 0,
    };
    /// 全 -1。
    pub const ALL_VULNERABLE: TriWord = TriWord {
        ones: 0,
        zeros: u32::MAX,
    };
    /// 全 0。
    pub const ALL_UNKNOWN: TriWord = TriWord {
        ones: 0,
        zeros: 0,
    };

    pub fn new(ones: u32, zeros: u32) -> Self {
        Self { ones, zeros }
    }

    /// 从 `Vec<Trit>` 打包（最多 32 个，多余截断）。
    pub fn pack(trits: &[Trit]) -> Self {
        let mut ones = 0u32;
        let mut zeros = 0u32;
        for (i, t) in trits.iter().enumerate().take(32) {
            match t {
                Trit::Hardened => ones |= 1 << i,
                Trit::Vulnerable => zeros |= 1 << i,
                Trit::Unknown => {}
            }
        }
        Self { ones, zeros }
    }

    /// 解包回 `Vec<Trit>`。
    pub fn unpack(&self) -> Vec<Trit> {
        (0..32)
            .map(|i| {
                let ones = (self.ones >> i) & 1;
                let zeros = (self.zeros >> i) & 1;
                match (ones, zeros) {
                    (1, 0) => Trit::Hardened,
                    (0, 1) => Trit::Vulnerable,
                    _ => Trit::Unknown,
                }
            })
            .collect()
    }

    /// K3 AND（逐位）：-1 支配，0 传染，两个 +1 才 +1。
    /// `ones = a.ones & b.ones`，`zeros = a.zeros | b.zeros`。
    pub fn and(&self, rhs: &Self) -> Self {
        Self {
            ones: self.ones & rhs.ones,
            zeros: self.zeros | rhs.zeros,
        }
    }

    /// K3 OR（逐位）：+1 支配，0 传染，两个 -1 才 -1。
    pub fn or(&self, rhs: &Self) -> Self {
        Self {
            ones: self.ones | rhs.ones,
            zeros: self.zeros & rhs.zeros,
        }
    }

    /// K3 NOT（逐位）：+1 ↔ -1，0 不变。
    pub fn not(&self) -> Self {
        Self {
            ones: self.zeros,
            zeros: self.ones,
        }
    }

    /// 统计 +1 的个数（popcount ones 位）。
    pub fn popcount_hardened(&self) -> u32 {
        self.ones.count_ones()
    }

    /// 统计 -1 的个数（popcount zeros 位）。
    pub fn popcount_vulnerable(&self) -> u32 {
        self.zeros.count_ones()
    }

    /// 统计 0 的个数。
    pub fn popcount_unknown(&self) -> u32 {
        32 - self.ones.count_ones() - self.zeros.count_ones()
    }

    /// 是否全部 +1。
    pub fn all_hardened(&self) -> bool {
        self.ones == u32::MAX && self.zeros == 0
    }

    /// 是否全部 0。
    pub fn all_unknown(&self) -> bool {
        self.ones == 0 && self.zeros == 0
    }

    /// 是否全部 -1。
    pub fn all_vulnerable(&self) -> bool {
        self.ones == 0 && self.zeros == u32::MAX
    }

    /// K3 AND 折叠：多个 TriWord 的逐位 AND。
    pub fn fold_and(words: &[Self]) -> Self {
        let mut acc = Self::ALL_HARDENED;
        for w in words {
            acc = acc.and(w);
        }
        acc
    }

    /// K3 OR 折叠。
    pub fn fold_or(words: &[Self]) -> Self {
        let mut acc = Self::ALL_UNKNOWN;
        for w in words {
            acc = acc.or(w);
        }
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trit::Trit;

    #[test]
    fn pack_unpack_roundtrip() {
        let trits = vec![
            Trit::Hardened,
            Trit::Unknown,
            Trit::Vulnerable,
            Trit::Hardened,
            Trit::Unknown,
        ];
        let w = TriWord::pack(&trits);
        let unpacked = w.unpack();
        for (i, t) in trits.iter().enumerate() {
            assert_eq!(unpacked[i], *t, "位置 {i} 不匹配");
        }
    }

    #[test]
    fn bitparallel_k3_matches_scalar() {
        // 穷举 3^4 = 81 种组合
        let all: [Trit; 3] = [Trit::Vulnerable, Trit::Unknown, Trit::Hardened];
        for a in all {
            for b in all {
                let wa = TriWord::pack(&[a]);
                let wb = TriWord::pack(&[b]);
                // AND
                let scalar_and = a.and(b);
                let parallel_and = wa.and(&wb);
                assert_eq!(
                    parallel_and.unpack()[0],
                    scalar_and,
                    "AND({a:?}, {b:?}) 不匹配"
                );
                // OR
                let scalar_or = a.or(b);
                let parallel_or = wa.or(&wb);
                assert_eq!(
                    parallel_or.unpack()[0],
                    scalar_or,
                    "OR({a:?}, {b:?}) 不匹配"
                );
                // NOT
                let scalar_not = a.not();
                let parallel_not = wa.not();
                assert_eq!(
                    parallel_not.unpack()[0],
                    scalar_not,
                    "NOT({a:?}) 不匹配"
                );
            }
        }
    }

    #[test]
    fn bulk_and_fold_32_trits_in_one_op() {
        // 32 个 trit 的 AND = 1 条位运算
        let words: Vec<TriWord> = (0..4)
            .map(|i| {
                if i == 2 {
                    TriWord::ALL_VULNERABLE
                } else {
                    TriWord::ALL_HARDENED
                }
            })
            .collect();
        let folded = TriWord::fold_and(&words);
        assert!(folded.all_vulnerable(), "任一 -1 应支配全部");
    }

    #[test]
    fn popcount_counts_correctly() {
        let trits: Vec<Trit> = (0..32)
            .map(|i| match i % 3 {
                0 => Trit::Hardened,
                1 => Trit::Unknown,
                _ => Trit::Vulnerable,
            })
            .collect();
        let w = TriWord::pack(&trits);
        assert_eq!(w.popcount_hardened(), 11, "11 个 +1");
        assert_eq!(w.popcount_vulnerable(), 10, "10 个 -1");
        assert_eq!(w.popcount_unknown(), 11, "11 个 0");
    }
}
