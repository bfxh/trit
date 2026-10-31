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
    pub const ALL_UNKNOWN: TriWord = TriWord { ones: 0, zeros: 0 };

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
                assert_eq!(parallel_not.unpack()[0], scalar_not, "NOT({a:?}) 不匹配");
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

// ── TriPack：变长三值序列的紧凑存储形态（5 trit/byte = 1.6 bit/trit） ───

/// TriPack 编解码错误（与上游 trit-codec 的 CodecError 同构，PR 分支可对拍）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackError {
    /// 字节落在 243..=255：不是合法的 5-trit 组合（3⁵=243 ≤ 256，余 13 非法值
    /// 全部用于显式报错而不是猜——零静默损坏）。
    InvalidByte(u8),
    /// 字节数与声明的 trit 长度不匹配。
    WrongByteCount { expected: usize, actual: usize },
}

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidByte(b) => write!(f, "非法字节 {b}：5 个 trit 只能表示 0..=242"),
            Self::WrongByteCount { expected, actual } => {
                write!(f, "字节数不符：期望 {expected} 实际 {actual}")
            }
        }
    }
}

impl std::error::Error for PackError {}

/// 变长三值序列打包：每 5 个连续 trit 压成 1 字节（**1.6 bit/trit**，
/// 比双导轨 TriWord 的 2 bit/trit 省 20%）。
///
/// 编码：槽位序 `v = t₀ + 3·t₁ + 9·t₂ + 27·t₃ + 81·t₄`（t 映射
/// Vulnerable=0 / Unknown=1 / Hardened=2），`v < 243` 落一字节；
/// 尾部不足 5 个用 `Unknown` 填满槽位（[`tri_unpack`] 按声明的 `len`
/// 截断，填充位永不外泄）。零依赖、确定性、可逆。
pub fn tri_pack(trits: &[Trit]) -> Vec<u8> {
    let digit = |t: Trit| match t {
        Trit::Vulnerable => 0u32,
        Trit::Unknown => 1,
        Trit::Hardened => 2,
    };
    trits
        .chunks(5)
        .map(|chunk| {
            let mut v: u32 = 0;
            let mut scale = 1u32;
            for &t in chunk {
                v += digit(t) * scale;
                scale *= 3;
            }
            v as u8
        })
        .collect()
}

/// [`tri_pack`] 的逆：按声明的 trit 长度解包。任何非法输入显式报错。
pub fn tri_unpack(bytes: &[u8], len: usize) -> Result<Vec<Trit>, PackError> {
    let expected = len.div_ceil(5);
    if bytes.len() != expected {
        return Err(PackError::WrongByteCount {
            expected,
            actual: bytes.len(),
        });
    }
    let mut out = Vec::with_capacity(len);
    for (bi, &b) in bytes.iter().enumerate() {
        if b >= 243 {
            return Err(PackError::InvalidByte(b));
        }
        let mut v = b as u32;
        let take = ((bi + 1) * 5).min(len) - bi * 5;
        for _ in 0..take {
            out.push(match v % 3 {
                0 => Trit::Vulnerable,
                1 => Trit::Unknown,
                _ => Trit::Hardened,
            });
            v /= 3;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod pack_tests {
    use super::*;

    #[test]
    fn pack_roundtrip_all_lengths() {
        for len in 0..24usize {
            // 周期模式覆盖三种值与全部相位
            let trits: Vec<Trit> = (0..len as u64)
                .map(|i| match i % 3 {
                    0 => Trit::Vulnerable,
                    1 => Trit::Unknown,
                    _ => Trit::Hardened,
                })
                .collect();
            let bytes = tri_pack(&trits);
            assert_eq!(tri_unpack(&bytes, len).unwrap(), trits, "len={len} 往返");
        }
    }

    /// 3⁵ 全空间：任意字节解出的 5 trit 重编回同一字节（合法域内）。
    #[test]
    fn full_5_trit_space_roundtrip() {
        for v in 0..243u8 {
            let mut v2 = v as u32;
            let mut trits = Vec::with_capacity(5);
            for _ in 0..5 {
                trits.push(match v2 % 3 {
                    0 => Trit::Vulnerable,
                    1 => Trit::Unknown,
                    _ => Trit::Hardened,
                });
                v2 /= 3;
            }
            assert_eq!(tri_pack(&trits)[0], v, "编码 = 解码 的逆（v={v}）");
        }
    }

    #[test]
    fn invalid_bytes_and_counts_are_rejected() {
        for b in 243u8..=255 {
            assert_eq!(
                tri_unpack(&[b], 5),
                Err(PackError::InvalidByte(b)),
                "13 个非法值全部报错"
            );
        }
        assert!(matches!(
            tri_unpack(&[0, 0], 5),
            Err(PackError::WrongByteCount {
                expected: 1,
                actual: 2
            })
        ));
    }

    #[test]
    fn packing_saves_20_percent_vs_dual_rail() {
        // 1000 trit：TriPack 200 字节（1.6 bit/trit，无块尾浪费）
        let trits = vec![Trit::Unknown; 1000];
        assert_eq!(tri_pack(&trits).len(), 200);
        // 双导轨理论 250 字节；TriWord 按 32-trit 块对齐 → 32 块 × 8B = 256 字节
        let words = trits.len().div_ceil(32);
        assert_eq!(words * 8, 256);
        assert!(tri_pack(&trits).len() < words * 8, "TriPack 更省");
        // 往返一致
        assert_eq!(tri_unpack(&tri_pack(&trits), 1000).unwrap(), trits);
    }

    #[test]
    fn agrees_with_triword_unpack() {
        // 同一序列：TriPack 与 TriWord 解包结果一致（两种存储形态同一语义）。
        // TriWord 固定 32-trit 容量，取前 32 个对拍。
        let trits: Vec<Trit> = (0..100u64)
            .map(|i| match i % 7 {
                0 | 3 => Trit::Vulnerable,
                1 | 5 => Trit::Hardened,
                _ => Trit::Unknown,
            })
            .collect();
        let from_triword = TriWord::pack(&trits[..32]).unpack();
        let from_pack = tri_unpack(&tri_pack(&trits[..32]), 32).unwrap();
        assert_eq!(from_triword, from_pack);
    }
}
