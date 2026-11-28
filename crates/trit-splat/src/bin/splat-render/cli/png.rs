//! RGB PNG 落盘：15/16 位深度另走 `slot`，这里只管 8-bit RGB 与色域标记。

use std::path::Path;

use trit_splat::Frame;
use trit_splat::color::{ColorConfig, ColorSpace};

pub fn write_png(path: &Path, frame: &Frame, color: &ColorConfig) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    if let Some([white, red, green, blue]) = color.output_gamut.primaries() {
        encoder.set_source_chromaticities(png::SourceChromaticities::new(white, red, green, blue));
        let gamma = match color.output {
            ColorSpace::Srgb => 45455,
            ColorSpace::Bt1886 => 41667,
            ColorSpace::Raw => 100000,
        };
        encoder.set_source_gamma(png::ScaledFloat::from_scaled(gamma));
    }
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&frame.rgb)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
#[path = "png/tests.rs"]
mod tests;
