//! 高压不变量：极端向量、跨字边界、大规模随机，以及**二进制基线在场**。
//!
//! 规则见 `docs/TERNARY-BOUNDARIES.md` §6/§7：对比必须带二进制基线，位运算结论必须扛住极端输入。

use trit_dot::{TernaryBits, bipolar_dot, float_dot, float_dot_4acc, pack_bipolar, scalar_dot};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn ternary(&mut self) -> i32 {
        (self.next() % 3) as i32 - 1
    }
    fn bipolar(&mut self) -> i32 {
        if self.next().is_multiple_of(2) { 1 } else { -1 }
    }
}

const LENGTHS: [usize; 8] = [1, 31, 32, 33, 63, 64, 65, 1000];

fn cases(len: usize) -> Vec<(&'static str, Vec<i32>)> {
    let all_pos = vec![1; len];
    let all_neg = vec![-1; len];
    let alternating = (0..len).map(|i| if i % 2 == 0 { 1 } else { -1 }).collect();
    let mut single_first = vec![0; len];
    let mut single_last = vec![0; len];
    if len > 0 {
        single_first[0] = 1;
        single_last[len - 1] = -1;
    }
    vec![
        ("all +1", all_pos),
        ("all -1", all_neg),
        ("alternating", alternating),
        ("single first", single_first),
        ("single last", single_last),
    ]
}

#[test]
fn ternary_dot_survives_adversarial_vectors() {
    for len in LENGTHS {
        for (name, a) in cases(len) {
            let b: Vec<i32> = (0..len)
                .map(|i| {
                    if i % 3 == 0 {
                        1
                    } else if i % 3 == 1 {
                        -1
                    } else {
                        0
                    }
                })
                .collect();
            let bits_a = TernaryBits::from_trits(&a);
            let bits_b = TernaryBits::from_trits(&b);
            let expected = scalar_dot(&a, &b);
            assert_eq!(
                bits_a.dot(&bits_b).expect("同长度"),
                expected,
                "{name} len={len}"
            );
            assert_eq!(
                bits_a.dot_sparse(&bits_b).expect("同长度"),
                expected,
                "{name} len={len} sparse"
            );
        }
    }
}

#[test]
fn bipolar_dot_handles_tail_padding() {
    // 尾部补 1 的约定在非 64 倍数长度上最容易写错。
    for len in [1usize, 2, 63, 64, 65, 127, 129] {
        for seed in 1..=8u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let a: Vec<i32> = (0..len).map(|_| rng.bipolar()).collect();
            let b: Vec<i32> = (0..len).map(|_| rng.bipolar()).collect();
            assert_eq!(
                bipolar_dot(&pack_bipolar(&a), &pack_bipolar(&b), len),
                scalar_dot(&a, &b),
                "len={len} seed={seed}"
            );
        }
    }
}

#[test]
fn large_random_matches_scalar_and_sparse_path() {
    for seed in [7u64, 11, 13] {
        let len = 1 << 20;
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let a: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
        let b: Vec<i32> = (0..len).map(|_| rng.ternary()).collect();
        let bits_a = TernaryBits::from_trits(&a);
        let bits_b = TernaryBits::from_trits(&b);
        let expected = scalar_dot(&a, &b);
        assert_eq!(
            bits_a.dot(&bits_b).expect("同长度"),
            expected,
            "seed={seed}"
        );
        assert_eq!(
            bits_a.dot_sparse(&bits_b).expect("同长度"),
            expected,
            "seed={seed}"
        );
    }
}

#[test]
fn sparse_skip_is_exact_when_blocks_are_empty() {
    let mut a = vec![0i32; 4096];
    let mut b = vec![0i32; 4096];
    for index in (0..4096).step_by(64) {
        a[index] = 1;
        b[index] = -1;
    }
    let bits_a = TernaryBits::from_trits(&a);
    let bits_b = TernaryBits::from_trits(&b);
    let expected = scalar_dot(&a, &b);
    assert_eq!(bits_a.dot(&bits_b).expect("同长度"), expected);
    assert_eq!(bits_a.dot_sparse(&bits_b).expect("同长度"), expected);
    assert_eq!(bits_a.nonzero_blocks(), 64);
}

#[test]
fn binary_baseline_is_present_and_consistent() {
    // 规则 §6：对比必须带二进制基线；这里顺便验证基线自己没算错。
    let len = 1 << 16;
    let mut rng = Rng(0xDEAD_BEEF);
    let a: Vec<f32> = (0..len).map(|_| rng.ternary_f32()).collect();
    let b: Vec<f32> = (0..len).map(|_| rng.ternary_f32()).collect();
    let naive = float_dot(&a, &b);
    let four = float_dot_4acc(&a, &b);
    let tolerance = naive.abs().max(1.0) * 1e-5;
    assert!(
        (naive - four).abs() <= tolerance,
        "f32 基线自身不一致：{naive} vs {four}"
    );
}

impl Rng {
    fn ternary_f32(&mut self) -> f32 {
        self.ternary() as f32
    }
}
