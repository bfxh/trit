//! 色温/色调白平衡：Tanner Helland 黑体近似，以 6500K 为中性。

mod kelvin;
mod settings;

use kelvin::kelvin_rgb;
pub use settings::WhiteBalance;

/// 色温/色调 → 线性 RGB 增益；以 6500K 为中性。
pub fn white_balance(temperature_k: f32, tint: f32) -> [f32; 3] {
    let target = kelvin_rgb(temperature_k.max(1.0));
    let neutral = kelvin_rgb(6500.0);
    let mut wb = [
        target[0] / neutral[0],
        target[1] / neutral[1],
        target[2] / neutral[2],
    ];
    let tint = tint.clamp(-1.0, 1.0);
    wb[1] *= 1.0 + 0.5 * tint;
    wb[0] *= 1.0 - 0.25 * tint;
    wb[2] *= 1.0 - 0.25 * tint;
    wb
}
