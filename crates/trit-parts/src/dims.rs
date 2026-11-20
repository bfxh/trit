//! 图尺寸 `w×h` 的统一上限：所有按像素分配的入口共用这一道。
//!
//! `w×h` 是整条生成链所有分配的根：`initial_noise(w*h)`、`vec![t; w*h]`、条件 one-hot 图。
//! 它在 `Denoiser::predict_x0`、`ddim_sample`、`ddim_sample_eps` 这些 pub API 上由调用方给出，
//! 以前一路裸乘进分配器：`w=h=100000` 就是 4e10 个 f32、160 GB，而且乘法本身干干净净。

/// 像素数上限：`1<<26`（约 6710 万）×4 byte ≈ 256 MB 一张图。
pub const MAX_PIXELS: usize = 1 << 26;

/// `w×h` 校验，返回像素数。
///
/// 先判溢出：`usize` 上 `w*h` 会绕回一个小数，后面的 `w*h - 1` 就指到错的像素；
/// `0` 也拒——`w*h - 1` 直接下溢。
pub fn check(w: usize, h: usize) -> Result<usize, String> {
    let n = w.checked_mul(h).ok_or("w×h 溢出")?;
    if n == 0 || n > MAX_PIXELS {
        return Err(format!("w×h 必须在 1..={MAX_PIXELS}（收到 {w}×{h}）"));
    }
    Ok(n)
}

/// `Err` 转 `panic!`：给返回 `Vec<f32>` 的 trait 方法用（`Denoiser::predict_x0` 的签名
/// 不能改成 `Result`），越界即契约违例。
pub fn pixels(w: usize, h: usize) -> usize {
    check(w, h).unwrap_or_else(|e| panic!("{e}"))
}
