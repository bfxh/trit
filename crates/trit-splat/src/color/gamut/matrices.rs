//! 原色转换矩阵：sRGB/Rec.709 到 Display P3 / Rec.2020（均为 D65 白点）。
use super::Gamut;
use super::map::perceptual;
pub(crate) fn convert(c: [f32; 3], gamut: Gamut) -> [f32; 3] {
    let dst = match gamut {
        Gamut::Srgb => c,
        Gamut::DisplayP3 => mul(SRGB_TO_P3, c),
        Gamut::Rec2020 => mul(SRGB_TO_2020, c),
    };
    perceptual(dst)
}

/// sRGB/Rec.709 → Display P3 原色（DCI-P3 原色 + D65 白点）。
const SRGB_TO_P3: [[f32; 3]; 3] = [
    [0.822_592_7, 0.177_533_95, 0.0],
    [0.033_199_6, 0.966_783_5, 0.0],
    [0.017_085_35, 0.072_395_74, 0.910_301_5],
];

/// sRGB/Rec.709 → Rec.2020 原色（BT.2020 原色 + D65 白点）。
const SRGB_TO_2020: [[f32; 3]; 3] = [
    [0.627_503_7, 0.329_275_46, 0.043_302_69],
    [0.069_108_39, 0.919_519_2, 0.011_359_59],
    [0.016_394_05, 0.088_011_25, 0.895_380_4],
];

fn mul(m: [[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}
