//! 两种三值打包格式。
//!
//! - [`Rail`]：双导轨位平面，每 trit 2 bit（`pos` / `neg` 各 1 bit），
//!   64 个 trit 一个 `u64`。适合 `AND + popcount`。
//! - [`Trytes`]：每字节 5 个 trit（`3^5 = 243 < 256`），1.6 bit/trit。
//!   适合把「5 个 trit 的加权和」做成查表。

use crate::trit::Trit;

/// 一个 `u64` 装多少个 trit。
pub const TRITS_PER_WORD: usize = 64;
/// 一个字节装多少个 trit（tryte）。
pub const TRITS_PER_BYTE: usize = 5;
/// tryte 的状态数。
pub const TRYTE_STATES: usize = 243;

/// 把一行 trit 打进 `bpr` 个字的位平面（`pos` / `neg`）。
///
/// 调用方保证 `pos` / `neg` 长度都是 `bpr`，且已清零。
pub fn pack_row(trits: &[Trit], pos: &mut [u64], neg: &mut [u64]) {
    for (i, t) in trits.iter().enumerate() {
        let word = i / TRITS_PER_WORD;
        let bit = i % TRITS_PER_WORD;
        match t.get() {
            1 => pos[word] |= 1u64 << bit,
            -1 => neg[word] |= 1u64 << bit,
            _ => {}
        }
    }
}

/// 双导轨三值向量。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rail {
    pos: Vec<u64>,
    neg: Vec<u64>,
    len: usize,
}

impl Rail {
    /// 从 trit 切片构造。尾部不足一个字的位保持为 0。
    pub fn from_trits(trits: &[Trit]) -> Self {
        let words = trits.len().div_ceil(TRITS_PER_WORD).max(1);
        let mut pos = vec![0u64; words];
        let mut neg = vec![0u64; words];
        pack_row(trits, &mut pos, &mut neg);
        Self {
            pos,
            neg,
            len: trits.len(),
        }
    }

    /// 从已有位平面构造并校验（长度一致、两导轨不重叠、尾部干净）。
    pub fn from_planes(pos: Vec<u64>, neg: Vec<u64>, len: usize) -> Option<Self> {
        if pos.len() != neg.len() || pos.len() != len.div_ceil(TRITS_PER_WORD).max(1) {
            return None;
        }
        if pos.iter().zip(&neg).any(|(p, n)| p & n != 0) {
            return None;
        }
        let used = len % TRITS_PER_WORD;
        if used != 0 {
            let mask = !0u64 << used;
            let last = pos.len() - 1;
            if (pos[last] | neg[last]) & mask != 0 {
                return None;
            }
        }
        Some(Self { pos, neg, len })
    }

    /// trit 个数。
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 字数。
    #[inline]
    pub fn words(&self) -> usize {
        self.pos.len()
    }

    /// 正导轨。
    #[inline]
    pub fn pos(&self) -> &[u64] {
        &self.pos
    }

    /// 负导轨。
    #[inline]
    pub fn neg(&self) -> &[u64] {
        &self.neg
    }

    /// 非零 trit 个数。
    pub fn nnz(&self) -> usize {
        self.pos
            .iter()
            .zip(&self.neg)
            .map(|(p, n)| (p | n).count_ones() as usize)
            .sum()
    }

    /// 零占比。
    pub fn sparsity(&self) -> f64 {
        if self.len == 0 {
            return 0.0;
        }
        1.0 - self.nnz() as f64 / self.len as f64
    }

    /// 精确三值点积（双导轨，4 组 `AND + popcount`）。
    ///
    /// 两个向量长度必须相同。
    pub fn dot(&self, other: &Rail) -> i64 {
        assert_eq!(self.len, other.len, "点积长度不一致");
        let mut acc = 0i64;
        for i in 0..self.pos.len() {
            let (ap, an) = (self.pos[i], self.neg[i]);
            let (bp, bn) = (other.pos[i], other.neg[i]);
            acc += (ap & bp).count_ones() as i64;
            acc += (an & bn).count_ones() as i64;
            acc -= (ap & bn).count_ones() as i64;
            acc -= (an & bp).count_ones() as i64;
        }
        acc
    }

    /// 标量参考点积，用于交叉验证。
    pub fn dot_scalar(&self, other: &Rail) -> i64 {
        let a = self.to_trits();
        let b = other.to_trits();
        a.iter()
            .zip(&b)
            .map(|(x, y)| (x.get() * y.get()) as i64)
            .sum()
    }

    /// 展开成 trit 向量。
    pub fn to_trits(&self) -> Vec<Trit> {
        let mut out = Vec::with_capacity(self.len);
        for i in 0..self.len {
            let word = self.pos[i / TRITS_PER_WORD];
            let bit = 1u64 << (i % TRITS_PER_WORD);
            let p = word & bit != 0;
            let n = self.neg[i / TRITS_PER_WORD] & bit != 0;
            out.push(match (p, n) {
                (true, false) => Trit::POS,
                (false, true) => Trit::NEG,
                _ => Trit::ZERO,
            });
        }
        out
    }

    /// 存储字节数（两个位平面）。
    pub fn bytes(&self) -> usize {
        self.pos.len() * 16
    }
}

/// tryte 打包：每字节 5 个 trit，按 `Σ (t_i + 1) * 3^i` 编码。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Trytes {
    data: Vec<u8>,
    len: usize,
}

impl Trytes {
    /// 从 trit 切片构造。
    pub fn from_trits(trits: &[Trit]) -> Self {
        let mut data = vec![0u8; trits.len().div_ceil(TRITS_PER_BYTE)];
        for (g, chunk) in trits.chunks(TRITS_PER_BYTE).enumerate() {
            data[g] = encode(chunk);
        }
        Self {
            data,
            len: trits.len(),
        }
    }

    /// 从已编码字节构造并校验（每个字节都必须 `< 243`）。
    pub fn from_codes(data: Vec<u8>, len: usize) -> Option<Self> {
        if data.len() != len.div_ceil(TRITS_PER_BYTE) {
            return None;
        }
        if data.iter().any(|&b| (b as usize) >= TRYTE_STATES) {
            return None;
        }
        Some(Self { data, len })
    }

    /// trit 个数。
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 字节数。
    #[inline]
    pub fn bytes(&self) -> usize {
        self.data.len()
    }

    /// 编码字节。
    #[inline]
    pub fn codes(&self) -> &[u8] {
        &self.data
    }

    /// 展开成 trit 向量。
    pub fn to_trits(&self) -> Vec<Trit> {
        let mut out = Vec::with_capacity(self.len);
        for (g, &code) in self.data.iter().enumerate() {
            let mut c = code as usize;
            for i in 0..TRITS_PER_BYTE {
                if g * TRITS_PER_BYTE + i >= self.len {
                    break;
                }
                let digit = c % 3;
                c /= 3;
                out.push(Trit::from_i8_unchecked(digit as i8 - 1));
            }
        }
        out
    }
}

/// 把最多 5 个 trit 编码成一个字节。
fn encode(chunk: &[Trit]) -> u8 {
    let mut code = 0usize;
    let mut pow = 1usize;
    for t in chunk {
        code += (t.get() as i32 + 1) as usize * pow;
        pow *= 3;
    }
    code as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(n: usize) -> Vec<Trit> {
        (0..n)
            .map(|i| Trit::from_i8_unchecked(((i * 7 + 3) % 3) as i8 - 1))
            .collect()
    }

    #[test]
    fn rail_roundtrips_at_odd_lengths() {
        for n in [0usize, 1, 5, 63, 64, 65, 127, 128, 200] {
            let v = sample(n);
            let rail = Rail::from_trits(&v);
            assert_eq!(rail.len(), n);
            assert_eq!(rail.to_trits(), v, "长度 {n}");
        }
    }

    #[test]
    fn rail_dot_matches_scalar() {
        for n in [1usize, 7, 64, 100, 256] {
            let a = Rail::from_trits(&sample(n));
            let b = Rail::from_trits(&sample(n).into_iter().rev().collect::<Vec<_>>());
            assert_eq!(a.dot(&b), a.dot_scalar(&b), "长度 {n}");
        }
    }

    #[test]
    fn rail_rejects_overlapping_planes() {
        assert!(Rail::from_planes(vec![1], vec![1], 1).is_none());
    }

    #[test]
    fn rail_rejects_dirty_tail() {
        assert!(Rail::from_planes(vec![1u64 << 63], vec![0], 1).is_none());
    }

    #[test]
    fn trytes_roundtrip_at_odd_lengths() {
        for n in [0usize, 1, 4, 5, 6, 11, 100, 576] {
            let v = sample(n);
            let t = Trytes::from_trits(&v);
            assert_eq!(t.len(), n);
            assert_eq!(t.bytes(), n.div_ceil(TRITS_PER_BYTE));
            assert_eq!(t.to_trits(), v, "长度 {n}");
        }
    }

    #[test]
    fn trytes_reject_out_of_range_code() {
        assert!(Trytes::from_codes(vec![243], 5).is_none());
    }

    #[test]
    fn tryte_code_is_base3() {
        // [-1,-1,-1,-1,-1] -> 0；[+1,+1,+1,+1,+1] -> 242
        let lo = Trytes::from_trits(&[Trit::NEG; 5]);
        let hi = Trytes::from_trits(&[Trit::POS; 5]);
        assert_eq!(lo.codes()[0], 0);
        assert_eq!(hi.codes()[0], 242);
    }

    #[test]
    fn sparsity_counts_zeros() {
        let v = [Trit::ZERO, Trit::POS, Trit::ZERO, Trit::NEG];
        let rail = Rail::from_trits(&v);
        assert_eq!(rail.nnz(), 2);
        assert!((rail.sparsity() - 0.5).abs() < 1e-12);
    }
}
