//! 输出 ICC 配置：标准组合使用自带紧凑 v4 矩阵/TRC 配置，非 sRGB 传递函数
//! 在共享 TRC 槽内改写为 16-byte `curv` gamma。

use super::{ColorSpace, Gamut};

static SRGB: &[u8] = include_bytes!("../../assets/icc/sRGB-v4.icc");
static DISPLAY_P3: &[u8] = include_bytes!("../../assets/icc/DisplayP3-v4.icc");
static REC2020_G24: &[u8] = include_bytes!("../../assets/icc/Rec2020-g24-v4.icc");

/// 按输出色域与传递函数选 ICC 配置；默认 sRGB/raw 返回 `None`，保持 v9 输出。
pub fn profile(gamut: Gamut, transfer: ColorSpace) -> Option<Vec<u8>> {
    let base = match (gamut, transfer) {
        (Gamut::Srgb, ColorSpace::Raw) => return None,
        (Gamut::Srgb, ColorSpace::Srgb) => SRGB,
        (Gamut::Srgb, ColorSpace::Bt1886) => return Some(with_gamma(SRGB, 2.4)),
        (Gamut::DisplayP3, ColorSpace::Srgb) => DISPLAY_P3,
        (Gamut::DisplayP3, ColorSpace::Bt1886) => return Some(with_gamma(DISPLAY_P3, 2.4)),
        (Gamut::DisplayP3, ColorSpace::Raw) => return Some(with_gamma(DISPLAY_P3, 1.0)),
        (Gamut::Rec2020, ColorSpace::Bt1886) => REC2020_G24,
        (Gamut::Rec2020, ColorSpace::Raw) => return Some(with_gamma(REC2020_G24, 1.0)),
        // Rec.2020 配 sRGB TRC 不是标准组合；保留 cHRM/gAMA，不写错误 ICC。
        (Gamut::Rec2020, ColorSpace::Srgb) => return None,
    };
    Some(base.to_vec())
}

/// 把三个共享 TRC 标签替换成 16-byte `curv` gamma；profile 其余字节不变。
fn with_gamma(base: &[u8], gamma: f32) -> Vec<u8> {
    let mut profile = base.to_vec();
    let count = u32::from_be_bytes(profile[128..132].try_into().unwrap()) as usize;
    let curve = gamma_curve(gamma);
    for i in 0..count {
        let entry = 132 + i * 12;
        let signature = &profile[entry..entry + 4];
        if signature == b"rTRC" || signature == b"gTRC" || signature == b"bTRC" {
            let offset = u32::from_be_bytes(profile[entry + 4..entry + 8].try_into().unwrap());
            let size = u32::from_be_bytes(profile[entry + 8..entry + 12].try_into().unwrap());
            assert!(size >= 16, "TRC 槽至少 16 字节");
            let offset = offset as usize;
            profile[offset..offset + 16].copy_from_slice(&curve);
            profile[entry + 8..entry + 12].copy_from_slice(&16u32.to_be_bytes());
        }
    }
    profile
}

fn gamma_curve(gamma: f32) -> [u8; 16] {
    let mut curve = [0u8; 16];
    curve[0..4].copy_from_slice(b"curv");
    curve[8..12].copy_from_slice(&1u32.to_be_bytes());
    let value = (gamma * 65_536.0).round() as u32;
    curve[12..16].copy_from_slice(&value.to_be_bytes());
    curve
}

#[cfg(test)]
#[path = "icc/tests.rs"]
mod tests;
