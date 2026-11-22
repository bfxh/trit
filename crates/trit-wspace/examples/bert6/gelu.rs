//! 精确 GELU 的 erf 近似（从 model.rs 拆出）。

pub(crate) fn gelu(value: f32) -> f32 {
    let x = f64::from(value);
    (0.5 * x * (1.0 + erf(x / std::f64::consts::SQRT_2))) as f32
}
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}
