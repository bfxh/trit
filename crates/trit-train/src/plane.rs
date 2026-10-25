//! 位平面 ALU：双导轨打包（1 trit = 2 bit），按字并行做逻辑与强加法。
//!
//! 打包布局与 `trit-ops` 一致：每 64 个 trit 一个 `u64`，`pos` 位为 1 表示 `+1`，
//! `neg` 位为 1 表示 `-1`，两位都 0 表示 `X`（未知）。
//!
//! 只做能按字并行的运算。**算术加法不做**：三进制进位在打包表示上只能逐 lane
//! 串行展开，此前 `trit-balanced` 的实测是比 i64 慢 2–3 个数量级，属于已证负结果。

use trit_core::Algebra;

/// 每个字的 trit 数。
pub const TRITS_PER_WORD: usize = 64;

/// 位平面向量。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planes {
    len: usize,
    pos: Vec<u64>,
    neg: Vec<u64>,
}

impl Planes {
    /// 全 `X` 的向量。
    pub fn new(len: usize) -> Self {
        let words = len.div_ceil(TRITS_PER_WORD);
        Self {
            len,
            pos: vec![0; words],
            neg: vec![0; words],
        }
    }

    /// 从 `-1/0/+1` 构造（其他值按 0 处理）。
    pub fn from_trits(values: &[i8]) -> Self {
        let mut out = Self::new(values.len());
        for (i, &v) in values.iter().enumerate() {
            let word = i / TRITS_PER_WORD;
            let bit = 1u64 << (i % TRITS_PER_WORD);
            match v {
                1 => out.pos[word] |= bit,
                -1 => out.neg[word] |= bit,
                _ => {}
            }
        }
        out
    }

    /// 展开成 `-1/0/+1`。
    pub fn to_trits(&self) -> Vec<i8> {
        let mut out = vec![0i8; self.len];
        for (i, slot) in out.iter_mut().enumerate() {
            let word = i / TRITS_PER_WORD;
            let bit = 1u64 << (i % TRITS_PER_WORD);
            if self.pos[word] & bit != 0 {
                *slot = 1;
            } else if self.neg[word] & bit != 0 {
                *slot = -1;
            }
        }
        out
    }

    /// 元素个数。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 字数。
    pub fn words(&self) -> usize {
        self.pos.len()
    }

    /// 只读的 `pos` 平面。
    pub fn pos_plane(&self) -> &[u64] {
        &self.pos
    }

    /// 只读的 `neg` 平面。
    pub fn neg_plane(&self) -> &[u64] {
        &self.neg
    }

    /// 逐位取反：交换两条导轨（`¬`）。
    pub fn not(&self) -> Self {
        Self {
            len: self.len,
            pos: self.neg.clone(),
            neg: self.pos.clone(),
        }
    }

    /// 逻辑与（`∧`），语义按 `alg`。
    pub fn and(&self, other: &Self, alg: Algebra) -> Self {
        self.combine(other, |ap, an, bp, bn, ax, bx| match alg {
            // Kleene 与 Łukasiewicz 的 `∧` 都是格序 min（X∧X = X）。
            Algebra::Kleene | Algebra::Lukasiewicz => (ap & bp, an | bn),
            // Bochvar：X 是毒值。
            Algebra::Bochvar => {
                let pos = ap & bp;
                (pos, !(pos | ax | bx))
            }
        })
    }

    /// 逻辑或（`∨`），语义按 `alg`。
    pub fn or(&self, other: &Self, alg: Algebra) -> Self {
        self.combine(other, |ap, an, bp, bn, ax, bx| match alg {
            Algebra::Kleene | Algebra::Lukasiewicz => (ap | bp, an & bn),
            Algebra::Bochvar => {
                let neg = an & bn;
                (!(neg | ax | bx), neg)
            }
        })
    }

    /// 强加法（`⊕`），语义按 `alg`。
    pub fn xor(&self, other: &Self, alg: Algebra) -> Self {
        self.combine(other, |ap, an, bp, bn, ax, bx| match alg {
            // Kleene / Bochvar：X 传播，两个 +1 才是 +1，否则 -1。
            Algebra::Kleene | Algebra::Bochvar => {
                let pos = ap & bp;
                (pos, !(pos | ax | bx))
            }
            // Łukasiewicz：min(1, a+b)，XX 与 TF 都为 +1。
            Algebra::Lukasiewicz => {
                let neg = an & bn;
                (!((an & (bn | bx)) | (ax & bn)), neg)
            }
        })
    }

    /// 按字组合两个向量；闭包拿到 `(ap, an, bp, bn, ax, bx)`，返回 `(pos, neg)`。
    fn combine(
        &self,
        other: &Self,
        f: impl Fn(u64, u64, u64, u64, u64, u64) -> (u64, u64),
    ) -> Self {
        assert_eq!(self.len, other.len, "位平面长度不一致");
        let mut pos = vec![0u64; self.words()];
        let mut neg = vec![0u64; self.words()];
        for (w, (p, n)) in pos.iter_mut().zip(neg.iter_mut()).enumerate() {
            let valid = self.valid_mask(w);
            let (ap, an) = (self.pos[w], self.neg[w]);
            let (bp, bn) = (other.pos[w], other.neg[w]);
            let ax = !(ap | an) & valid;
            let bx = !(bp | bn) & valid;
            let (out_pos, out_neg) = f(ap, an, bp, bn, ax, bx);
            *p = out_pos & valid;
            *n = out_neg & valid;
        }
        Self {
            len: self.len,
            pos,
            neg,
        }
    }

    /// 最后一字的有效位掩码（长度外的位必须保持 0）。
    fn valid_mask(&self, word: usize) -> u64 {
        let tail = self.len % TRITS_PER_WORD;
        if word + 1 < self.words() || tail == 0 {
            u64::MAX
        } else {
            (1u64 << tail) - 1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_core::value::Ternary;

    fn scalar(v: i8) -> Ternary {
        match v {
            -1 => Ternary::False,
            0 => Ternary::Unknown,
            _ => Ternary::True,
        }
    }

    fn sample(len: usize, seed: u64) -> Vec<i8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state % 3) as i8 - 1
            })
            .collect()
    }

    #[test]
    fn round_trip() {
        for len in [0usize, 1, 63, 64, 65, 130] {
            let values = sample(len, 7 + len as u64);
            let planes = Planes::from_trits(&values);
            assert_eq!(planes.to_trits(), values, "len={len}");
        }
    }

    #[test]
    fn exhaustive_binary_ops_match_scalar() {
        let values = [-1i8, 0, 1];
        for alg in Algebra::ALL {
            for &a in &values {
                for &b in &values {
                    let pa = Planes::from_trits(&[a]);
                    let pb = Planes::from_trits(&[b]);
                    let (ta, tb) = (scalar(a), scalar(b));
                    assert_eq!(
                        pa.and(&pb, alg).to_trits(),
                        [trit(alg.and(ta, tb))],
                        "AND {a},{b} under {alg:?}"
                    );
                    assert_eq!(
                        pa.or(&pb, alg).to_trits(),
                        [trit(alg.or(ta, tb))],
                        "OR {a},{b} under {alg:?}"
                    );
                    assert_eq!(
                        pa.xor(&pb, alg).to_trits(),
                        [trit(alg.strong_add(ta, tb))],
                        "XOR {a},{b} under {alg:?}"
                    );
                }
            }
        }
    }

    fn trit(value: Ternary) -> i8 {
        match value {
            Ternary::False => -1,
            Ternary::Unknown => 0,
            Ternary::True => 1,
        }
    }

    #[test]
    fn not_matches_scalar_and_is_involutive() {
        let values = sample(130, 11);
        let planes = Planes::from_trits(&values);
        let negated = planes.not();
        let expected: Vec<i8> = values.iter().map(|&v| -v).collect();
        assert_eq!(negated.to_trits(), expected);
        assert_eq!(negated.not(), planes);
    }

    #[test]
    fn tail_bits_stay_clean() {
        let values = sample(65, 3);
        let planes = Planes::from_trits(&values);
        let out = planes.and(&planes, Algebra::Kleene);
        // 第二字只有 1 个有效位。
        assert_eq!(out.pos_plane()[1] & !1, 0);
        assert_eq!(out.neg_plane()[1] & !1, 0);
    }

    #[test]
    fn long_vectors_match_scalar() {
        let a = sample(200, 5);
        let b = sample(200, 9);
        let pa = Planes::from_trits(&a);
        let pb = Planes::from_trits(&b);
        for alg in Algebra::ALL {
            let got = pa.and(&pb, alg).to_trits();
            let want: Vec<i8> = a
                .iter()
                .zip(&b)
                .map(|(&x, &y)| trit(alg.and(scalar(x), scalar(y))))
                .collect();
            assert_eq!(got, want, "AND under {alg:?}");
        }
    }
}
