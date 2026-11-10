//! 位平面 ALU：双导轨打包（1 trit = 2 bit），按字并行做逻辑与强加法。
//!
//! 打包布局与 `trit-ops` 一致：每 64 个 trit 一个 `u64`，`pos` 位为 1 表示 `+1`，
//! `neg` 位为 1 表示 `-1`，两位都 0 表示 `X`（未知）。
//!
//! 只做能按字并行的运算。**算术加法不做**：三进制进位在打包表示上只能逐 lane
//! 串行展开，此前 `trit-balanced` 的实测是比 i64 慢 2–3 个数量级，属于已证负结果。

mod alu;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use trit_core::Algebra;
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
        // 第二字只有 1 个有效位。逐代数都查：Bochvar 的补运算会把无效位填成 1，
        // 只测 Kleene 自与的话掩码是不是空转看不出来（实测那样会漏掉 valid_mask 的退化）。
        for alg in Algebra::ALL {
            for out in [
                planes.and(&planes, alg),
                planes.or(&planes, alg),
                planes.xor(&planes, alg),
            ] {
                assert_eq!(
                    out.pos_plane()[1] & !1,
                    0,
                    "{alg:?} 的 pos 导轨尾部污染了：{:064b}",
                    out.pos_plane()[1]
                );
                assert_eq!(
                    out.neg_plane()[1] & !1,
                    0,
                    "{alg:?} 的 neg 导轨尾部污染了：{:064b}",
                    out.neg_plane()[1]
                );
            }
        }
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
