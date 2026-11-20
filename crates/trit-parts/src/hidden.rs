//! `matnet`（`material-cnn`）的输入边长与隐藏宽度上限。
//!
//! 这个数是 [`crate::matnet::MatNet`] 的通道数，会平方级放大：
//! `Conv::new(hidden, hidden)` 内部是 `cout*cin*9 = hidden²*9` 个 f32，
//! `w`/`mw`/`vw` 各一份。校验必须在分配之前，不能留在 `Conv::new` 里。

/// 输入边长（正方形裁剪）。
pub const SIDE: usize = 24;

/// 隐藏宽度上限：`256²×9` 个 f32 = 2.3 MB/份（w/mw/vw 三份 = 7 MB）。
pub const MAX_HIDDEN: usize = 256;

/// 隐藏宽度校验。`0` 也拒：池化与线性头会全空，前向时除零。
pub fn check(h: usize) -> Result<(), String> {
    if h == 0 {
        return Err("隐藏层宽度不能为 0".into());
    }
    if h > MAX_HIDDEN {
        return Err(format!("隐藏层宽度 {h} 超过上限 {MAX_HIDDEN}"));
    }
    Ok(())
}
