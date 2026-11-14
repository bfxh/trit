//! 分组 scale 之后，行为判等还能不能穷举？
//!
//! 仓库 `Layer` 的 `scales` 是**每行一个正数**，于是有"正 scale 不改行为"这一不变性
//! （符号输出只看 `sign(Σ trit·input)`，乘以正 scale 不变）。穷举判等正建立在这上面。
//!
//! `GroupedLayer` 的分组**跨行**，同一行内不同列可以落到不同 scale 上。本测试验证：
//!
//! 1. 跨行分组**会改变**符号输出——即"行缩放不变性"在分组结构下失效；
//! 2. 但在 trits / groups / scales **全部固定**时，行为仍是确定的、可穷举的。

use trit_wspace::grouped::GroupedLayer;

fn sign(value: f32) -> i32 {
    if value > 1e-9 {
        1
    } else if value < -1e-9 {
        -1
    } else {
        0
    }
}

#[test]
fn cross_row_grouping_changes_sign_output() {
    // 2×2，trits 完全相同：[[1, 1], [1, -1]]
    let trits = [1i8, 1, 1, -1];
    let bias = [0.0f32, 0.0];

    // A：每行一个 scale（等价于 `Layer` 的语义）
    let per_row = GroupedLayer::from_trits(2, 2, &trits, &[0, 0, 1, 1], &[0.1, 10.0], &bias)
        .expect("per-row");

    // B：跨行分组（同一行内两列落到不同 scale）
    let cross_row = GroupedLayer::from_trits(2, 2, &trits, &[0, 1, 0, 1], &[0.1, 10.0], &bias)
        .expect("cross-row");

    // 输入 [1, 1]：A 的第 1 行是 10 - 10 = 0（符号 0）；B 是 0.1 - 10 = -9.9（符号 -1）
    let input = [1.0f32, 1.0];
    let a = per_row.forward(&input).expect("a");
    let b = cross_row.forward(&input).expect("b");
    assert_eq!(sign(a[1]), 0, "per-row 第 1 行应为 0，实际 {a:?}");
    assert_eq!(sign(b[1]), -1, "cross-row 第 1 行应为负，实际 {b:?}");

    // 穷举 9 个输入，统计两者符号行为不同的个数
    let mut differing = 0usize;
    for x0 in [-1.0f32, 0.0, 1.0] {
        for x1 in [-1.0f32, 0.0, 1.0] {
            let input = [x0, x1];
            let left = per_row.forward(&input).expect("left");
            let right = cross_row.forward(&input).expect("right");
            let left_sign: Vec<i32> = left.iter().map(|v| sign(*v)).collect();
            let right_sign: Vec<i32> = right.iter().map(|v| sign(*v)).collect();
            if left_sign != right_sign {
                differing += 1;
            }
        }
    }
    assert!(
        differing > 0,
        "跨行分组应当至少在一个输入上改变符号行为（实际 0 个）"
    );
}

#[test]
fn per_row_grouping_keeps_row_scale_invariance() {
    // 每行一个 scale 时，把某一行的 scale 整体放大不改变该行输出的符号。
    let trits = [1i8, -1, 1, 1];
    let bias = [0.0f32, 0.0];
    let groups = [0, 0, 1, 1];
    let small = GroupedLayer::from_trits(2, 2, &trits, &groups, &[1.0, 1.0], &bias).expect("small");
    let large = GroupedLayer::from_trits(2, 2, &trits, &groups, &[3.0, 7.0], &bias).expect("large");

    for x0 in [-1.0f32, 0.0, 1.0] {
        for x1 in [-1.0f32, 0.0, 1.0] {
            let input = [x0, x1];
            let a: Vec<i32> = small
                .forward(&input)
                .expect("a")
                .iter()
                .map(|v| sign(*v))
                .collect();
            let b: Vec<i32> = large
                .forward(&input)
                .expect("b")
                .iter()
                .map(|v| sign(*v))
                .collect();
            assert_eq!(a, b, "每行正 scale 缩放不应改变符号行为，输入 {input:?}");
        }
    }
}

#[test]
fn behavior_is_enumerable_once_scale_is_fixed() {
    // trits / groups / scales 全固定时，行为在 3^n 输入上完全确定 ⇒ 可穷举判等。
    let layer = GroupedLayer::from_trits(
        2,
        2,
        &[1, 0, -1, 1],
        &[0, 1, 0, 1],
        &[0.5, 2.0],
        &[0.0, 0.0],
    )
    .expect("layer");

    let mut signatures = Vec::new();
    for x0 in [-1.0f32, 0.0, 1.0] {
        for x1 in [-1.0f32, 0.0, 1.0] {
            let out = layer.forward(&[x0, x1]).expect("forward");
            signatures.push(out.iter().map(|v| sign(*v)).collect::<Vec<i32>>());
        }
    }
    assert_eq!(signatures.len(), 9, "3^2 = 9 个输入应全部可枚举");

    // 再算一遍必须逐位相同（确定性）
    let mut again = Vec::new();
    for x0 in [-1.0f32, 0.0, 1.0] {
        for x1 in [-1.0f32, 0.0, 1.0] {
            let out = layer.forward(&[x0, x1]).expect("forward");
            again.push(out.iter().map(|v| sign(*v)).collect::<Vec<i32>>());
        }
    }
    assert_eq!(signatures, again, "固定 scale 下行为必须确定");
}
