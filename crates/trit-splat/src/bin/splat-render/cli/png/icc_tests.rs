//! PNG iCCP 嵌入测试。

use super::write_png;
use std::fs::File;
use trit_splat::Frame;
use trit_splat::color::{ColorConfig, ColorSpace, Gamut};

fn render(color: ColorConfig) -> png::Reader<File> {
    let path = std::env::temp_dir().join(format!(
        "trit-splat-icc-{}-{:?}.png",
        std::process::id(),
        std::thread::current().id()
    ));
    let frame = Frame {
        width: 1,
        height: 1,
        rgb: vec![255, 0, 0],
    };
    write_png(&path, &frame, &color).unwrap();
    let reader = png::Decoder::new(File::open(&path).unwrap())
        .read_info()
        .unwrap();
    let _ = std::fs::remove_file(path);
    reader
}

#[test]
fn display_p3_png_embeds_v4_profile() {
    let reader = render(ColorConfig {
        output_gamut: Gamut::DisplayP3,
        output: ColorSpace::Srgb,
        ..ColorConfig::default()
    });
    let profile = reader.info().icc_profile.as_deref().expect("P3 应写 iCCP");
    assert_eq!(&profile[36..40], b"acsp");
    assert_eq!(&profile[16..20], b"RGB ");
    assert_eq!(
        profile.len() as u32,
        u32::from_be_bytes(profile[0..4].try_into().unwrap())
    );
}
