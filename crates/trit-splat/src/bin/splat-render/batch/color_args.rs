//! `splat-render` 的色彩参数解析：曝光、色温/色调、饱和度、tone map 与输入/输出空间。

use crate::cli;
use trit_splat::color::{ColorConfig, ColorSpace, Gamut, ToneMap, WhiteBalance};
#[path = "color_args/parse.rs"]
mod parse;
use parse::{number, space};

pub(super) fn parse(argv: &[String]) -> Result<ColorConfig, String> {
    let exposure_ev = number(argv, "--exposure")?.unwrap_or(0.0);
    if !exposure_ev.is_finite() {
        return Err(format!("--exposure 必须有限: {exposure_ev}"));
    }
    let temperature_k = number(argv, "--temperature")?.unwrap_or(0.0);
    if temperature_k != 0.0 && !(1000.0..=40_000.0).contains(&temperature_k) {
        return Err(format!(
            "--temperature 需为 0 或 1000..40000 K: {temperature_k}"
        ));
    }
    let tint = number(argv, "--tint")?.unwrap_or(0.0);
    if !(-1.0..=1.0).contains(&tint) {
        return Err(format!("--tint 需在 [-1,1]: {tint}"));
    }
    let saturation = number(argv, "--saturation")?.unwrap_or(1.0);
    if !(0.0..=4.0).contains(&saturation) {
        return Err(format!("--saturation 需在 [0,4]: {saturation}"));
    }
    let tone = cli::value(argv, "--tone-map")
        .unwrap_or_else(|| "none".into())
        .to_ascii_lowercase();
    let tone_map = match tone.as_str() {
        "none" => ToneMap::None,
        "reinhard" => ToneMap::Reinhard,
        "aces" => ToneMap::Aces,
        _ => return Err(format!("--tone-map 只支持 none|reinhard|aces: {tone}")),
    };
    Ok(ColorConfig {
        input: space(argv, "--color-in", ColorSpace::Raw)?,
        output: space(argv, "--color-out", ColorSpace::Raw)?,
        output_gamut: parse::gamut(argv, "--gamut", Gamut::Srgb)?,
        exposure_ev,
        white_balance: WhiteBalance {
            temperature_k,
            tint,
        },
        saturation,
        tone_map,
    })
}
