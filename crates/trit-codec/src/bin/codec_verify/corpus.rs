//! 确定性三值语料：9 种形态 × 25 个长度，种子上限固定，**同种子逐位可复现**。
//!
//! 语料不是基准集，只求覆盖边界与病态分布：空、全同值、交替、稀疏、成段、随机。

use trit_core::value::Ternary;

/// 默认种子；命令行 `--seed` 可换，但报告里会原样记下来。
pub const SEED_DEFAULT: u64 = 0x9e37_79b9_7f4a_7c15;

/// 一种样本：名字（形态/长度）与 trit 序列。
pub struct Sample {
    pub name: String,
    pub values: Vec<Ternary>,
}

/// 形态名（报告里按名字定位失败样本）。
pub const KINDS: [&str; 9] = [
    "zeros", "ones", "unknowns", "alt01", "alt0x", "alt1x", "sparse", "runs", "random",
];

/// 全量长度档：含 5 trits/byte 的字节边界（4/5/6、10/11、24/25/26）与大步长。
const LENGTHS: [usize; 25] = [
    0, 1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 24, 25, 26, 49, 50, 51, 127, 128, 129, 255, 256, 257,
    1024,
];

/// 快速档：只保留边界与一个中档长度。
const LENGTHS_QUICK: [usize; 8] = [0, 1, 5, 6, 9, 17, 65, 257];

/// 极小 xorshift64：只用来造语料，不承担密码学用途。
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 { 0 } else { self.next_u64() % n }
    }
}

/// 按形态生成 `len` 个 trit。
fn pattern(kind: usize, len: usize, rng: &mut Rng) -> Vec<Ternary> {
    let mut out = Vec::new();
    let mut run_value = Ternary::False;
    for index in 0..len {
        let value = match kind {
            0 => Ternary::False,
            1 => Ternary::True,
            2 => Ternary::Unknown,
            3 => {
                if index % 2 == 0 {
                    Ternary::False
                } else {
                    Ternary::True
                }
            }
            4 => {
                if index % 2 == 0 {
                    Ternary::False
                } else {
                    Ternary::Unknown
                }
            }
            5 => {
                if index % 2 == 0 {
                    Ternary::True
                } else {
                    Ternary::Unknown
                }
            }
            6 => {
                // 稀疏：约 3% 是有定义值，其余 Unknown。
                if rng.below(100) < 3 {
                    if rng.below(2) == 0 {
                        Ternary::False
                    } else {
                        Ternary::True
                    }
                } else {
                    Ternary::Unknown
                }
            }
            7 => {
                // 成段：每 5 个换一次值，压 RLE。
                if index % 5 == 0 {
                    run_value = Ternary::from_index(rng.below(3) as usize);
                }
                run_value
            }
            _ => Ternary::from_index(rng.below(3) as usize),
        };
        out.push(value);
    }
    out
}

/// 生成语料：种子先混入形态号与长度，保证每个样本的流互不相同、又完全确定。
pub fn samples(quick: bool, seed: u64) -> Vec<Sample> {
    let lengths: &[usize] = if quick { &LENGTHS_QUICK } else { &LENGTHS };
    let mut out = Vec::new();
    for (kind, kind_name) in KINDS.iter().enumerate() {
        for &len in lengths {
            let mixed = seed
                .wrapping_add(kind as u64 * 0x9e37_79b9)
                .wrapping_add(len as u64 * 0x85eb_ca6b);
            let mut rng = Rng::new(mixed);
            out.push(Sample {
                name: format!("{kind_name}/{len}"),
                values: pattern(kind, len, &mut rng),
            });
        }
    }
    out
}

/// 语料规模读数（样本数、总 trit 数），进报告头。
pub fn stats(samples: &[Sample]) -> (usize, usize) {
    let trits = samples.iter().map(|s| s.values.len()).sum();
    (samples.len(), trits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_identical_corpus() {
        let a = samples(true, SEED_DEFAULT);
        let b = samples(true, SEED_DEFAULT);
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.values, y.values, "{}", x.name);
        }
    }

    #[test]
    fn different_seed_changes_random_shape_only_where_expected() {
        let a = samples(true, SEED_DEFAULT);
        let b = samples(true, SEED_DEFAULT ^ 0xdead_beef);
        let zeros_a = &a[0].values;
        let zeros_b = &b[0].values;
        assert_eq!(zeros_a, zeros_b, "全 False 形态不该受种子影响");
        let random_a = a.last().expect("非空").values.clone();
        let random_b = b.last().expect("非空").values.clone();
        assert_ne!(random_a, random_b, "随机形态应随种子变化");
    }

    #[test]
    fn pattern_lengths_are_exact_for_every_shape() {
        let s = samples(false, SEED_DEFAULT);
        let lengths = LENGTHS;
        for (index, sample) in s.iter().enumerate() {
            assert_eq!(sample.values.len(), lengths[index % lengths.len()]);
        }
    }
}
