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
        let gamma = if color.output == ColorSpace::Srgb {
            45455
        } else {
            100000
        };
        encoder.set_source_gamma(png::ScaledFloat::from_scaled(gamma));
    }
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer
        .write_image_data(&frame.rgb)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use trit_splat::color::Gamut;

    #[test]
    fn p3_png_writes_chromaticities_and_gamma() {
        let path = std::env::temp_dir().join(format!(
            "trit-splat-p3-{}-{:?}.png",
            std::process::id(),
            std::thread::current().id()
        ));
        let frame = Frame {
            width: 1,
            height: 1,
            rgb: vec![255, 0, 0],
        };
        let color = ColorConfig {
            output_gamut: Gamut::DisplayP3,
            output: ColorSpace::Srgb,
            ..ColorConfig::default()
        };
        write_png(&path, &frame, &color).unwrap();
        let reader = png::Decoder::new(File::open(&path).unwrap())
            .read_info()
            .unwrap();
        let info = reader.info();
        let c = info.source_chromaticities.expect("P3 应写 cHRM");
        assert_eq!(c.white.0.into_scaled(), 31270);
        assert_eq!(c.red.0.into_scaled(), 68000);
        assert_eq!(
            info.source_gamma.expect("P3 应写 gAMA").into_scaled(),
            45455
        );
        let _ = std::fs::remove_file(path);
    }
}
