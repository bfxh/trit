//! TL 查表数学层的契约：27/4 组合与索引布局。
//! 边界：只复算“每个组合对应的线性组合值”，不实现 bitnet.cpp 的 kernel 内 byte 布局。

use trit_core::Ternary;
use trit_export::bitnet::tl::{build_three_table, build_two_table, three_index, two_index};

#[test]
fn three_table_matches_linear_combination() {
    let weights = [0.5f32, -0.25, 1.0];
    let scale = 2.0;
    let table = build_three_table(weights, scale);
    assert_eq!(table.len(), 27);
    let t = |d: usize| match d {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    };
    for idx in 0..27 {
        let d = [idx / 9, (idx / 3) % 3, idx % 3];
        let q = d.map(|v| v as f32 - 1.0);
        let dot: f32 = weights.iter().zip(q).map(|(w, v)| w * v).sum();
        assert_eq!(three_index(d.map(t)), idx);
        assert_eq!(table[idx], (scale * dot).round() as i8, "idx={idx}");
    }
}

#[test]
fn two_table_matches_linear_combination() {
    let weights = [0.75f32, -1.5];
    let scale = 4.0;
    let table = build_two_table(weights, scale);
    assert_eq!(table.len(), 4);
    for a in 0..2 {
        for b in 0..2 {
            let idx = two_index([a == 1, b == 1]);
            let q = [a as f32 * 2.0 - 1.0, b as f32 * 2.0 - 1.0];
            let expected = (scale * (weights[0] * q[0] + weights[1] * q[1])).round() as i8;
            assert_eq!(table[idx], expected, "idx={idx}");
        }
    }
}

#[test]
fn index_layout_is_ternary_and_binary() {
    let (f, u, t) = (Ternary::False, Ternary::Unknown, Ternary::True);
    assert_eq!((three_index([f, u, t]), three_index([t, u, f])), (5, 21));
    assert_eq!((two_index([false, true]), two_index([true, false])), (1, 2));
}

#[test]
fn zero_weights_yield_zero_table() {
    assert!(build_three_table([0.0; 3], 8.0).iter().all(|v| *v == 0));
    assert!(build_two_table([0.0; 2], 8.0).iter().all(|v| *v == 0));
}
