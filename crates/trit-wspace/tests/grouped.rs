//! 分组三值层：行为一致性、位宽、组号覆盖。

use trit_wspace::grouped::GroupedLayer;
use trit_wspace::trit::Trit;

fn sample() -> (usize, usize, Vec<f32>, Vec<f32>) {
    // 3×4，幅度跨 3 个数量级（正是分组量化要解决的问题）。
    let weights = vec![
        0.5, -0.02, 0.003, 0.1, //
        -0.4, 0.08, -0.001, 0.02, //
        0.3, -0.06, 0.0005, -0.15,
    ];
    let bias = vec![0.1, -0.2, 0.0];
    (3, 4, weights, bias)
}

#[test]
fn forward_matches_effective_weights() {
    let (rows, cols, weights, bias) = sample();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 4).expect("quantize");
    let input = vec![1.0f32, -2.0, 0.5, 3.0];
    let got = layer.forward(&input).expect("forward");
    // 用等效 f32 权重手算
    let effective = layer.to_f32();
    let mut want = bias.clone();
    for row in 0..rows {
        for col in 0..cols {
            want[row] += effective[row * cols + col] * input[col];
        }
    }
    for (a, b) in got.iter().zip(&want) {
        assert!((a - b).abs() < 1e-5, "forward {got:?} vs {want:?}");
    }
}

#[test]
fn quantization_stays_close_to_original() {
    let (rows, cols, weights, bias) = sample();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 4).expect("quantize");
    let effective = layer.to_f32();
    // 三值化必然有误差；只要求不高于"统一 per-tensor scale"的水平。
    let mean_abs = weights.iter().map(|v| v.abs()).sum::<f32>() / weights.len() as f32;
    let max_error = weights
        .iter()
        .zip(&effective)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_error <= mean_abs * 1.5,
        "最大误差 {max_error} 超过 1.5·E|w| ({mean_abs})"
    );
}

#[test]
fn every_element_has_a_group_and_scale() {
    let (rows, cols, weights, bias) = sample();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 2).expect("quantize");
    assert_eq!(layer.group_count(), (rows * cols).div_ceil(2));
    for row in 0..rows {
        for col in 0..cols {
            let group = layer.group_of(row, col).expect("group");
            assert!((group as usize) < layer.group_count());
            assert!(layer.scale(group as usize).is_some());
            assert!(layer.trit(row, col).is_some());
        }
    }
}

#[test]
fn bits_per_weight_includes_group_ids() {
    let (rows, cols, weights, bias) = sample();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 4).expect("quantize");
    let groups = layer.group_count();
    // 2（trit）+ 32/4（scale）+ log2(组数)（组号）
    let expected = 2.0 + 32.0 / 4.0 + (groups as f32).log2();
    assert!(
        (layer.bits_per_weight() - expected).abs() < 1e-5,
        "位宽 {} != {expected}",
        layer.bits_per_weight()
    );
    // 组号开销必须被计入——否则会低估位宽（本项目踩过这个坑）。
    assert!(layer.bits_per_weight() > 2.0 + 32.0 / 4.0);
}

#[test]
fn rejects_bad_shapes() {
    assert!(GroupedLayer::quantize(0, 4, &[], &[], 4).is_err());
    assert!(GroupedLayer::quantize(2, 2, &[1.0, 2.0], &[0.0, 0.0], 2).is_err());
    assert!(GroupedLayer::quantize(2, 2, &[1.0, 2.0, 3.0, 4.0], &[0.0], 2).is_err());
    let layer = GroupedLayer::quantize(2, 2, &[1.0, 2.0, 3.0, 4.0], &[0.0, 0.0], 2).expect("ok");
    assert!(layer.forward(&[1.0]).is_err());
    assert!(layer.trit(5, 0).is_none());
}

#[test]
fn zero_input_gives_bias() {
    let (rows, cols, weights, bias) = sample();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 4).expect("quantize");
    let out = layer.forward(&vec![0.0; cols]).expect("forward");
    for (got, want) in out.iter().zip(&bias) {
        assert!((got - want).abs() < 1e-6);
    }
}

#[test]
fn trits_are_in_the_ternary_set() {
    let (rows, cols, weights, bias) = sample();
    let layer = GroupedLayer::quantize(rows, cols, &weights, &bias, 3).expect("quantize");
    for row in 0..rows {
        for col in 0..cols {
            let trit = layer.trit(row, col).expect("trit");
            assert!(Trit::ALL.contains(&trit));
        }
    }
}
