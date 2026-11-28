//! 感知色域映射：把目标原色线性 RGB 中的超色域颜色压到 RGB 立方体边界。
//!
//! 以 Rec.709 亮度为中性轴，只压缩色度、保持亮度，避免逐通道截断造成偏色。
//! 这是输出侧近似，不是 ICC 感知 intent 的全色域压缩。

const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];

pub(in crate::color) fn perceptual(c: [f32; 3]) -> [f32; 3] {
    let y = LUMA[0] * c[0] + LUMA[1] * c[1] + LUMA[2] * c[2];
    if !y.is_finite() {
        return [0.0; 3];
    }
    if y <= 0.0 {
        return [0.0; 3];
    }
    if y >= 1.0 {
        return [1.0; 3];
    }
    let mut scale = 1.0f32;
    for &v in &c {
        let d = v - y;
        if d > 0.0 {
            scale = scale.min((1.0 - y) / d);
        } else if d < 0.0 {
            scale = scale.min(y / -d);
        }
    }
    if scale >= 1.0 {
        return c.map(|v| v.clamp(0.0, 1.0));
    }
    c.map(|v| (y + (v - y) * scale).clamp(0.0, 1.0))
}
