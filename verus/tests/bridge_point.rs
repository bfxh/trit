//! `TriVec` 的单点操作（`get` / `try_set` / `from_ternary`）与标量语义对拍。
//!
//! Verus 侧（`verus/src/point.rs`、`point/proofs.rs`）证的是「结果视图 == 标量
//! 语义」；这里对拍**主库实现**，覆盖边界长度（0/1/63/64/65/130）、越界错误、
//! padding 位与 unknown 平面归一化（全零 ⇒ `None`）。

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
/// 单点读：`get`/`try_get` 与标量取值一致，越界返回错误。
#[test]
fn tri_vec_get_matches_scalar() {
    for len in LENS {
        let values = sample(len, 13);
        let v = TriVec::from_ternary(&values);
        for i in 0..len {
            assert_eq!(v.get(i), values[i], "get 在 len={len} i={i} 不一致");
            assert_eq!(
                v.try_get(i).unwrap(),
                values[i],
                "try_get 在 len={len} i={i}"
            );
        }
        assert!(v.try_get(len).is_err(), "len={len} 的越界读未报错");
    }
}

/// 单点写：`try_set` 与标量替换一致，且清空 Unknown 后 `unknowns()` 归一为 `None`。
#[test]
fn tri_vec_set_matches_scalar() {
    let targets = [Ternary::False, Ternary::True, Ternary::Unknown];
    for len in LENS {
        let values = sample(len, 17);
        for i in 0..len {
            for &t in &targets {
                let mut v = TriVec::from_ternary(&values);
                v.try_set(i, t).unwrap();
                let mut expected = values.clone();
                expected[i] = t;
                for j in 0..len {
                    assert_eq!(
                        v.get(j),
                        expected[j],
                        "set(len={len}, i={i}, {t:?}) 在 j={j} 不一致"
                    );
                }
                let has_unknown = expected.contains(&Ternary::Unknown);
                assert_eq!(
                    v.unknowns().is_some(),
                    has_unknown,
                    "set(len={len}, i={i}, {t:?}) 的 unknown 平面归一化不对"
                );
            }
        }
        let mut v = TriVec::from_ternary(&values);
        assert!(
            v.try_set(len, Ternary::True).is_err(),
            "len={len} 的越界写未报错"
        );
    }
}

/// `from_ternary` 的打包结果：逐位取值一致，Unknown 平面按需归一。
#[test]
fn tri_vec_from_ternary_roundtrip() {
    for len in LENS {
        let values = sample(len, 19);
        let v = TriVec::from_ternary(&values);
        for i in 0..len {
            assert_eq!(
                v.get(i),
                values[i],
                "from_ternary 在 len={len} i={i} 不一致"
            );
        }
        assert_eq!(
            v.unknowns().is_some(),
            values.contains(&Ternary::Unknown),
            "from_ternary 在 len={len} 的 unknown 平面归一化不对"
        );
    }
}
