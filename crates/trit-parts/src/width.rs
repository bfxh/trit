//! `--width` 的合法区间与通道缩放。
//!
//! 独立成一层是为了不挂在 `features = ["gpu"]` 上：`net` 模块整块要 `wgpu`，
//! 而这里只是两个数相乘 —— 校验必须能被不装 GPU 的测试与 CI 覆盖。

/// `--width` 的上限：通道按它线性放大，放大后再算 `kk * kk * cin * cout`
/// 决定权重个数。`4` 已经让 c128 变成 512（单层 `9 × 512 × 512` = 2.4M 个 f32）。
pub const MAX_WIDTH: f32 = 4.0;

/// 基准通道数 `c` 按 `width` 放大，向上取到 4 的倍数。
///
/// `f32 as u32` 是饱和转换：`c × width` 超过 `u32::MAX` 时取到 `u32::MAX`，
/// `div_ceil(4) * 4` 再绕回 `0`，`.max(4)` 把结果塌成 4 —— 所以调用方必须先
/// [`check`]，否则整张网会静默变成 4 通道。
pub fn scaled_channels(c: u32, width: f32) -> u32 {
    let v = (c as f32 * width).round() as u32;
    (v.div_ceil(4) * 4).max(4)
}

/// `--width` 必须在 `(0, MAX_WIDTH]` 内且有限。
pub fn check(width: f32) -> Result<(), String> {
    if !width.is_finite() || width <= 0.0 || width > MAX_WIDTH {
        return Err(format!("宽度 {width} 不在 (0, {MAX_WIDTH}] 内"));
    }
    Ok(())
}
