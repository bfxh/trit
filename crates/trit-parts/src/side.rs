//! 方形边长参数（`--size` 这类）的统一上限。
//!
//! 这个数是所有下游分配的根：`n*n*3` 个像素、`vec![0.0f32; w*h]`、
//! `initial_noise(w*h)`。4296² 就已经是 200 MB 一张图，再大是拿内存换分辨率。

/// 边长上限：`4096²×3` 个 f32 ≈ 200 MB。
pub const MAX_SIDE: usize = 4096;

/// 边长校验。`0` 也拒：`n*n` 会得到空图，`seam_ratio` 之类直接除零。
pub fn check(n: usize) -> Result<(), String> {
    if !(1..=MAX_SIDE).contains(&n) {
        return Err(format!("--size 必须在 1..={MAX_SIDE}（收到 {n}）"));
    }
    Ok(())
}
