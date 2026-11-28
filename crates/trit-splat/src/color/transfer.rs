//! 传递函数：sRGB 与 BT.1886（gamma 2.4）。

/// sRGB 编码：线性光 → sRGB。
pub fn srgb_encode(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB 解码：sRGB → 线性光。
pub fn srgb_decode(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// BT.1886 编码：线性光 → gamma 2.4（Lb=0 简化）。
pub fn bt1886_encode(v: f32) -> f32 {
    v.max(0.0).powf(1.0 / 2.4)
}

/// BT.1886 解码：gamma 2.4 → 线性光（Lb=0 简化）。
pub fn bt1886_decode(v: f32) -> f32 {
    v.max(0.0).powf(2.4)
}
