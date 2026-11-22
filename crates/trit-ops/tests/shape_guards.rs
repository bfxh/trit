//! `gemv_*` 是 `pub fn`，入口必须在建裸切片之前拒绝错形状（#588）。
//!
//! 修复前这些用例会走进 `from_raw_parts` 按谎报长度发布的视图：
//! AVX2 路径 SIGSEGV，标量回退静默截断给出错值。

use trit_ops::bench::{gemv_f16, gemv_f32, gemv_q4};

#[test]
fn correct_shape_still_computes() {
    let (rows, cols) = (1usize, 64usize);
    let w: Vec<f32> = (0..rows * cols).map(|i| (i % 3) as f32 - 1.0).collect();
    let x: Vec<f32> = (0..cols).map(|i| (i % 5) as f32 * 0.1).collect();
    let mut out = vec![0.0f32; rows];
    gemv_f32(&w, rows, cols, &x, &mut out);
    let expected: f32 = w.iter().zip(&x).map(|(a, b)| a * b).sum();
    assert!((out[0] - expected).abs() < 1e-4);
}

#[test]
#[should_panic(expected = "权重长度必须是 rows*cols")]
fn short_f32_weights_are_rejected() {
    let (rows, cols) = (1usize, 1usize << 18);
    let w = vec![0f32; 1];
    let x = vec![1f32; cols];
    let mut out = vec![0f32; rows];
    gemv_f32(&w, rows, cols, &x, &mut out);
}

#[test]
#[should_panic(expected = "激活长度必须是 cols")]
fn short_f32_activation_is_rejected() {
    let (rows, cols) = (1usize, 8usize);
    let w = vec![0f32; rows * cols];
    let x = vec![1f32; cols - 1];
    let mut out = vec![0f32; rows];
    gemv_f32(&w, rows, cols, &x, &mut out);
}

#[test]
#[should_panic(expected = "权重长度必须是 rows*cols")]
fn short_f16_weights_are_rejected() {
    let (rows, cols) = (2usize, 64usize);
    let w = vec![0u16; 1];
    let x = vec![1f32; cols];
    let mut out = vec![0f32; rows];
    gemv_f16(&w, rows, cols, &x, &mut out);
}

#[test]
#[should_panic(expected = "Q4_0 权重长度必须是 rows*每行字节数")]
fn short_q4_weights_are_rejected() {
    let (rows, cols) = (1usize, 32usize);
    let w = vec![0u8; 1];
    let x = vec![1f32; cols];
    let mut out = vec![0f32; rows];
    gemv_q4(&w, rows, cols, &x, &mut out);
}

#[test]
#[should_panic(expected = "cols 是 32 的倍数")]
fn q4_requires_block_aligned_cols() {
    let (rows, cols) = (1usize, 40usize);
    let w = vec![0u8; rows * cols.div_ceil(32) * 18];
    let x = vec![1f32; cols];
    let mut out = vec![0f32; rows];
    gemv_q4(&w, rows, cols, &x, &mut out);
}

#[test]
#[should_panic(expected = "rows*cols 溢出 usize")]
fn overflowing_shape_is_rejected() {
    let w = vec![0f32; 1];
    let x = vec![1f32; 1];
    let mut out = vec![0f32; 1];
    gemv_f32(&w, usize::MAX, 2, &x, &mut out);
}
