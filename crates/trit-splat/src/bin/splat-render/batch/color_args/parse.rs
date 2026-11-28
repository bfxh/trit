//! 色彩参数的底层解析：数字、传递函数与色域枚举。

use trit_splat::color::{ColorSpace, Gamut};

use crate::cli;

#[path = "parse/number.rs"]
mod number;

pub(super) use number::number;
pub(super) fn space(
    argv: &[String],
    flag: &str,
    default: ColorSpace,
) -> Result<ColorSpace, String> {
    let Some(v) = cli::value(argv, flag) else {
        return Ok(default);
    };
    match v.to_ascii_lowercase().as_str() {
        "raw" => Ok(ColorSpace::Raw),
        "srgb" => Ok(ColorSpace::Srgb),
        "bt1886" => Ok(ColorSpace::Bt1886),
        _ => Err(format!("{flag} 只支持 raw|srgb|bt1886: {v}")),
    }
}

pub(super) fn gamut(argv: &[String], flag: &str, default: Gamut) -> Result<Gamut, String> {
    let Some(v) = cli::value(argv, flag) else {
        return Ok(default);
    };
    match v.to_ascii_lowercase().as_str() {
        "srgb" => Ok(Gamut::Srgb),
        "p3" | "display-p3" => Ok(Gamut::DisplayP3),
        "2020" | "rec2020" | "bt2020" => Ok(Gamut::Rec2020),
        _ => Err(format!("{flag} 只支持 srgb|p3|rec2020: {v}")),
    }
}
