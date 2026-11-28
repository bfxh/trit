//! 输出原色色域：源按 sRGB/Rec.709 原色解释，线性光转到目标原色。
//!
//! Display P3 使用 DCI-P3 原色与 D65 白点，传递函数与 sRGB 相同；Rec.2020 使用
//! BT.2020 原色与 D65 白点，8-bit SDR 输出应配 BT.1886（`ColorSpace::Bt1886`）。
//! 转换矩阵由 sRGB/Rec.709 与目标原色的 RGB→XYZ 矩阵（D65）合成，无需色适应。

mod matrices;

pub(super) use matrices::convert;

/// 输出原色色域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gamut {
    /// sRGB / Rec.709 原色；恒等，不做转换。
    Srgb,
    /// Display P3 原色（DCI-P3 原色 + D65 白点）。
    DisplayP3,
    /// Rec.2020 原色（BT.2020 原色 + D65 白点）。
    Rec2020,
}

impl Gamut {
    /// PNG cHRM 标记用的原色坐标：白点、红、绿、蓝；sRGB 返回 `None`。
    pub fn primaries(self) -> Option<[(f32, f32); 4]> {
        match self {
            Self::Srgb => None,
            Self::DisplayP3 => Some([
                (0.3127, 0.3290),
                (0.680, 0.320),
                (0.265, 0.690),
                (0.150, 0.060),
            ]),
            Self::Rec2020 => Some([
                (0.3127, 0.3290),
                (0.708, 0.292),
                (0.170, 0.797),
                (0.131, 0.046),
            ]),
        }
    }
}
