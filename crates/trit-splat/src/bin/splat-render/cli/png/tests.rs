//! RGB PNG 色域标记测试。

use super::*;
use std::fs::File;
use trit_splat::color::Gamut;

fn write_red_png(name: &str, gamut: Gamut, output: ColorSpace) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "trit-splat-{name}-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    let frame = Frame {
        width: 1,
        height: 1,
        rgb: vec![255, 0, 0],
    };
    let color = ColorConfig {
        output_gamut: gamut,
        output,
        ..ColorConfig::default()
    };
    write_png(&path, &frame, &color).unwrap();
    path
}

fn read_png(path: &std::path::Path) -> png::Reader<File> {
    png::Decoder::new(File::open(path).unwrap())
        .read_info()
        .unwrap()
}

#[test]
fn p3_png_writes_chromaticities_and_gamma() {
    let path = write_red_png("p3", Gamut::DisplayP3, ColorSpace::Srgb);
    let reader = read_png(&path);
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

#[test]
fn rec2020_png_writes_chromaticities_and_gamma() {
    let path = write_red_png("rec2020", Gamut::Rec2020, ColorSpace::Bt1886);
    let reader = read_png(&path);
    let info = reader.info();
    let c = info.source_chromaticities.expect("Rec.2020 应写 cHRM");
    assert_eq!(c.white.0.into_scaled(), 31270);
    assert_eq!(c.red.0.into_scaled(), 70800);
    assert_eq!(
        info.source_gamma.expect("Rec.2020 应写 gAMA").into_scaled(),
        41667
    );
    let _ = std::fs::remove_file(path);
}
