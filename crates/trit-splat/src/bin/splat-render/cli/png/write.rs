//! RGB PNG 落盘：15/16 位深度另走 `slot`，这里只管 8-bit RGB、色域标记与 ICC。

use std::path::Path;

use trit_splat::Frame;
use trit_splat::color::{ColorConfig, ColorSpace, icc_profile};

pub fn write_png(path: &Path, frame: &Frame, color: &ColorConfig) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut info = png::Info::with_size(frame.width, frame.height);
    info.color_type = png::ColorType::Rgb;
    info.bit_depth = png::BitDepth::Eight;
    if let Some([white, red, green, blue]) = color.output_gamut.primaries() {
        info.source_chromaticities = Some(png::SourceChromaticities::new(white, red, green, blue));
        let gamma = match color.output {
            ColorSpace::Srgb => 45455,
            ColorSpace::Bt1886 => 41667,
            ColorSpace::Raw => 100000,
        };
        info.source_gamma = Some(png::ScaledFloat::from_scaled(gamma));
    }
    info.icc_profile = icc_profile(color.output_gamut, color.output).map(Into::into);
    let encoder = png::Encoder::with_info(file, info).map_err(|e| format!("建 PNG 编码器: {e}"))?;
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&frame.rgb)
        .map_err(|e| e.to_string())
}
