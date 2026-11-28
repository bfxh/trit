//! 色彩参数的底层解析：数字、传递函数与色域枚举。

use trit_splat::color::{ColorSpace, Gamut};

use crate::cli;

pub(super) fn number(argv: &[String], flag: &str) -> Result<Option<f32>, String> {
    cli::value(argv, flag)
        .map(|v| {
            v.parse::<f32>()
                .map_err(|_| format!("{flag} 非法数字: {v}"))
        })
        .transpose()
}

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
        _ => Err(format!("{flag} 只支持 raw|srgb: {v}")),
    }
}

pub(super) fn gamut(argv: &[String], flag: &str, default: Gamut) -> Result<Gamut, String> {
    let Some(v) = cli::value(argv, flag) else {
        return Ok(default);
    };
    match v.to_ascii_lowercase().as_str() {
        "srgb" => Ok(Gamut::Srgb),
        "p3" | "display-p3" => Ok(Gamut::DisplayP3),
        _ => Err(format!("{flag} 只支持 srgb|p3: {v}")),
    }
}
