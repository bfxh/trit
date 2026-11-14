//! `Layer::project`：三值基投影（输入维度压缩）。
//!
//! 三条不变量：恒等基不动、形状按 k 缩减、列数不匹配必须报错。

use trit_wspace::Layer;

fn layer(rows: usize, cols: usize, values: &[i8]) -> Layer {
    Layer::new(rows, cols, values, &vec![0; rows]).expect("layer")
}

#[test]
fn identity_basis_keeps_layer() {
    let mat = layer(2, 3, &[1, -1, 0, 0, 1, 1]);
    let basis = layer(3, 3, &[1, 0, 0, 0, 1, 0, 0, 0, 1]);
    let out = mat.project(&basis).expect("project");
    assert_eq!((out.rows(), out.cols()), (2, 3));
    for row in 0..2 {
        for col in 0..3 {
            assert_eq!(out.trit(row, col), mat.trit(row, col), "({row},{col})");
        }
    }
}

#[test]
fn project_reduces_input_dim() {
    let mat = layer(2, 4, &[1, 1, -1, -1, -1, 1, 1, -1]);
    let basis = layer(2, 4, &[1, 1, 0, 0, 0, 0, 1, 1]);
    let out = mat.project(&basis).expect("project");
    assert_eq!((out.rows(), out.cols()), (2, 2));
    // 第 0 行：前两列和 = 2 → +1；后两列和 = -2 → -1
    assert_eq!(out.trit(0, 0).map(|t| t.to_i8()), Some(1));
    assert_eq!(out.trit(0, 1).map(|t| t.to_i8()), Some(-1));
}

#[test]
fn project_rejects_shape_mismatch() {
    let mat = layer(1, 3, &[1, 0, -1]);
    let basis = layer(2, 2, &[1, 0, 0, 1]);
    assert!(mat.project(&basis).is_err());
}

#[test]
fn project_preserves_bias() {
    let mat = Layer::new(2, 3, &[1, 0, -1, -1, 1, 1], &[1, -1]).expect("layer");
    let basis = layer(1, 3, &[1, 0, 0]);
    let out = mat.project(&basis).expect("project");
    assert_eq!(out.bias(), mat.bias());
}
