//! 第 37 轮 S1 回归：Q4_0 打包与解包在 `cols % 32 != 0` 时的长度口径必须一致。
//!
//! 修复前：`pack_q4` 按全局 32 元素切块（floor），`gemv_q4` 按每行 `ceil(cols/32)*18`
//! 取块，且 `quantize_act_q8` 也按 floor 出块——三者不一致，`from_raw_parts` 越界读，
//! `bench --rows 1 --cols 1` 直接 SIGSEGV。

use trit_ops::bench::{gemv_q4, pack_q4};

#[test]
fn pack_q4_size_matches_row_padding() {
    for cols in [1usize, 31, 32, 33, 64] {
        let w: Vec<f32> = (0..cols).map(|i| i as f32 * 0.1 - 1.0).collect();
        assert_eq!(
            pack_q4(&w, 1, cols).len(),
            cols.div_ceil(32) * 18,
            "单行 cols={cols}"
        );
    }
    // 多行：每行独立补齐
    let (rows, cols) = (3usize, 33usize);
    let w: Vec<f32> = (0..rows * cols).map(|i| i as f32 * 0.01).collect();
    assert_eq!(
        pack_q4(&w, rows, cols).len(),
        rows * cols.div_ceil(32) * 18,
        "多行 rows={rows} cols={cols}"
    );
}

#[test]
fn gemv_q4_stays_in_bounds_when_cols_is_not_a_multiple_of_32() {
    for cols in [1usize, 33] {
        let w: Vec<f32> = (0..cols).map(|i| (i % 3) as f32 - 1.0).collect();
        let packed = pack_q4(&w, 1, cols);
        let x = vec![1.0f32; cols];
        let mut out = vec![0.0f32; 1];
        gemv_q4(&packed, 1, cols, &x, &mut out);
        assert!(out[0].is_finite(), "cols={cols} 不应越界读或产出非有限值");
    }
}
