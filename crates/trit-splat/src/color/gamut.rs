//! 输出原色色域：源按 sRGB/Rec.709 原色解释，线性光转到目标原色。
//!
//! Display P3 使用 DCI-P3 原色与 D65 白点，传递函数与 sRGB 相同；转换矩阵由
//! sRGB/Rec.709 与 Display P3 的 RGB→XYZ 矩阵（D65）合成，无需色适应。

/// 输出原色色域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gamut {
    /// sRGB / Rec.709 原色；恒等，不做转换。
    Srgb,
    /// Display P3 原色（DCI-P3 原色 + D65 白点）。
    DisplayP3,
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
        }
    }
}

pub(super) fn convert(c: [f32; 3], gamut: Gamut) -> [f32; 3] {
    match gamut {
        Gamut::Srgb => c,
        Gamut::DisplayP3 => mul(SRGB_TO_P3, c),
    }
}

/// sRGB/Rec.709 → Display P3 原色（均为 D65 白点，无颜色适应）。
///
/// 由 sRGB 的 RGB→XYZ 矩阵左乘 Display P3 RGB→XYZ 的逆矩阵得到。
const SRGB_TO_P3: [[f32; 3]; 3] = [
    [0.822_592_7, 0.177_533_95, 0.0],
    [0.033_199_6, 0.966_783_5, 0.0],
    [0.017_085_35, 0.072_395_74, 0.910_301_5],
];

fn mul(m: [[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}
