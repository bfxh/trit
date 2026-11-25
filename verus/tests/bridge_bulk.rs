//! `TriVec`（位切片三值向量）与标量 `Ternary` 的逐元素对拍。
//!
//! Verus 侧（`verus/src/bulk.rs`、`bulk/proofs.rs`）证明的是平面公式与标量
//! 运算逐位等价；这里对拍**主库实现**，覆盖边界长度（0/1/63/64/65/130）
//! 与三种代数（K3 / Ł3 / B3），把公式与真实代码连起来。

use trit_core::algebra::Algebra;
use trit_core::{Ternary, TriVec};

/// 确定性三值样本（LCG，与主库单测一致的构造方式）。
fn sample(len: usize, seed: u64) -> Vec<Ternary> {
    let mut x = seed;
    (0..len)
        .map(|_| {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            match (x >> 32) % 3 {
                0 => Ternary::False,
                1 => Ternary::True,
                _ => Ternary::Unknown,
            }
        })
        .collect()
}

const LENS: [usize; 6] = [0, 1, 63, 64, 65, 130];

#[test]
fn tri_vec_not_matches_scalar() {
    for len in LENS {
        let values = sample(len, 3);
        let mut v = TriVec::from_ternary(&values);
        v.not();
        for (i, &value) in values.iter().enumerate() {
            assert_eq!(
                v.get(i),
                Algebra::Kleene.not(value),
                "not 在 len={len} i={i} 不一致"
            );
        }
    }
}

#[test]
fn tri_vec_and_or_match_scalar() {
    for alg in Algebra::ALL {
        for len in LENS {
            let a = sample(len, 7);
            let b = sample(len, 11);
            let vb = TriVec::from_ternary(&b);

            let mut va = TriVec::from_ternary(&a);
            va.and_assign(&vb, alg).unwrap();
            for i in 0..len {
                assert_eq!(
                    va.get(i),
                    alg.and(a[i], b[i]),
                    "{alg:?} and 在 len={len} i={i} 不一致"
                );
            }

            let mut vo = TriVec::from_ternary(&a);
            vo.or_assign(&vb, alg).unwrap();
            for i in 0..len {
                assert_eq!(
                    vo.get(i),
                    alg.or(a[i], b[i]),
                    "{alg:?} or 在 len={len} i={i} 不一致"
                );
            }
        }
    }
}

/// `len` 之外的 padding 位必须被清零，且不影响 `len` 内的取值。
#[test]
fn tri_vec_padding_is_masked() {
    for len in [1usize, 3, 63, 64, 65] {
        let words = len.div_ceil(64);
        let mut v = TriVec::from_binary(vec![u64::MAX; words], len);
        v.not();
        for i in 0..len {
            assert_eq!(v.get(i), Ternary::False, "not 在 len={len} i={i}");
        }
        // 尾部 padding 位应保持 0，使 `is_binary` 与相等语义稳定。
        assert!(v.is_binary(), "len={len} 的 padding 位未清零");
    }
}

/// 纯二值输入与含 `Unknown` 输入在 B3 下分别走不同平面路径。
#[test]
fn tri_vec_binary_and_unknown_mix() {
    let a = TriVec::from_bools(&[true, false, true]);
    let b = TriVec::from_ternary(&[Ternary::Unknown, Ternary::True, Ternary::False]);

    let mut and_b3 = a.clone();
    and_b3.and_assign(&b, Algebra::Bochvar).unwrap();
    assert_eq!(and_b3.get(0), Ternary::Unknown);
    assert_eq!(and_b3.get(1), Ternary::False);
    assert_eq!(and_b3.get(2), Ternary::False);

    let mut or_b3 = a.clone();
    or_b3.or_assign(&b, Algebra::Bochvar).unwrap();
    assert_eq!(or_b3.get(0), Ternary::Unknown);
    assert_eq!(or_b3.get(1), Ternary::True);
    assert_eq!(or_b3.get(2), Ternary::True);
}
