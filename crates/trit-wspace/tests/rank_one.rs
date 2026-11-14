//! 秩一编辑：精确性、方向性、边界。

use trit_wspace::rank_one::{apply, changed_signs, edit_cosine, edit_error, rank_one_update};

fn matrix(rows: usize, cols: usize) -> Vec<f32> {
    (0..rows * cols)
        .map(|index| ((index as f32) * 0.37).sin() * 0.5)
        .collect()
}

#[test]
fn rank_one_makes_output_exact() {
    let (rows, cols) = (4usize, 3usize);
    let mut weight = matrix(rows, cols);
    let x = vec![0.3f32, -0.7, 0.5];
    let target = vec![1.0f32, -2.0, 0.5, 3.0];

    let before = edit_error(&weight, rows, cols, &x, &target);
    assert!(before > 0.1, "编辑前误差应该很大（否则测试构造失效）");

    rank_one_update(&mut weight, rows, cols, &x, &target).expect("update");
    let after = edit_error(&weight, rows, cols, &x, &target);
    assert!(after < 1e-5, "编辑后误差应接近 0，实际 {after}");
    assert!((edit_cosine(&weight, rows, cols, &x, &target) - 1.0).abs() < 1e-5);
}

#[test]
fn rank_one_only_changes_the_edited_direction() {
    // 秩一只沿 x 方向加东西：对与 x 正交的 y，W'y = Wy 必须完全不变。
    let (rows, cols) = (3usize, 2usize);
    let mut weight = matrix(rows, cols);
    let x = vec![1.0f32, 0.0];
    let y = vec![0.0f32, 1.0];
    let before = apply(&weight, rows, cols, &y);
    rank_one_update(&mut weight, rows, cols, &x, &vec![2.0, 3.0, 4.0]).expect("update");
    let after = apply(&weight, rows, cols, &y);
    for (a, b) in before.iter().zip(&after) {
        assert!(
            (a - b).abs() < 1e-6,
            "正交方向应不受影响：{before:?} vs {after:?}"
        );
    }
}

#[test]
fn rank_one_changes_values_densely_but_few_signs() {
    // 秩一更新改动整行的**数值**（增量为 scale·x，满行都变）；但**符号翻转是稀疏的**——
    // 只有幅度接近增量大小的权重才会变号。
    //
    // 这个区分很关键：三值化只保留符号，所以它会丢掉大部分编辑效果，
    // 即使"整行数值都变了"。
    let (rows, cols) = (8usize, 16usize);
    let mut weight = matrix(rows, cols);
    let before = weight.clone();
    let x: Vec<f32> = (0..cols).map(|i| ((i as f32) * 0.7).cos()).collect();
    let target: Vec<f32> = (0..rows).map(|i| ((i as f32) * 1.3).sin() * 2.0).collect();
    rank_one_update(&mut weight, rows, cols, &x, &target).expect("update");

    // 数值：几乎每个元素都变了
    let moved = before
        .iter()
        .zip(&weight)
        .filter(|(a, b)| (*a - *b).abs() > 1e-6)
        .count();
    assert_eq!(
        moved,
        rows * cols,
        "秩一更新的增量为 scale·x，整行数值都应变"
    );

    // 符号：翻转只占少数
    let changed = changed_signs(&before, &weight);
    assert!(changed > 0, "应至少翻转一些符号（否则说明增量太小）");
    assert!(
        changed < rows * cols / 4,
        "符号翻转应远少于整行（实测 {changed} / {}）",
        rows * cols
    );
}

#[test]
fn rank_one_rejects_bad_shapes() {
    let mut weight = vec![0.0f32; 6];
    assert!(
        rank_one_update(&mut weight, 2, 3, &[1.0, 0.0], &[1.0, 2.0]).is_err(),
        "x 长度错"
    );
    assert!(
        rank_one_update(&mut weight, 2, 3, &[1.0, 0.0, 0.0], &[1.0]).is_err(),
        "target 长度错"
    );
    assert!(
        rank_one_update(&mut weight, 2, 4, &[1.0, 0.0, 0.0], &[1.0, 2.0]).is_err(),
        "权重形状错"
    );
    assert!(
        rank_one_update(&mut weight, 2, 3, &[0.0, 0.0, 0.0], &[1.0, 2.0]).is_err(),
        "零输入应拒绝"
    );
    assert!(
        rank_one_update(&mut weight, 0, 3, &[1.0, 0.0, 0.0], &[]).is_err(),
        "空层应拒绝"
    );
}

#[test]
fn apply_matches_manual_product() {
    let (rows, cols) = (2usize, 3usize);
    let weight = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let x = vec![0.5f32, -1.0, 2.0];
    let out = apply(&weight, rows, cols, &x);
    // 行 0: 1*0.5 + 2*(-1) + 3*2 = 4.5 ; 行 1: 4*0.5 + 5*(-1) + 6*2 = 9.0
    assert!((out[0] - 4.5).abs() < 1e-6);
    assert!((out[1] - 9.0).abs() < 1e-6);
}
