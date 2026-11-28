//! Tanner Helland 黑体色温近似。

/// 输入色温 K，输出 0..1 线性 RGB。
pub(super) fn kelvin_rgb(k: f32) -> [f32; 3] {
    let t = (k / 100.0).clamp(10.0, 400.0);
    let r = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let g = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_17 * (t - 60.0).powf(-0.075_514_85)
    };
    let b = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [r, g, b].map(|v| (v / 255.0).clamp(0.0, 1.0))
}
