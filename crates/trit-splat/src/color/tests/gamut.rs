//! 色域单测：Display P3 原色转换与 cHRM 原色坐标。

use crate::Frame;
use crate::color::{ColorConfig, Gamut};

fn frame(rgb: &[u8]) -> Frame {
    Frame {
        width: (rgb.len() / 3) as u32,
        height: 1,
        rgb: rgb.to_vec(),
    }
}

#[test]
fn p3_red_is_converted_from_srgb() {
    let mut f = frame(&[255, 0, 0]);
    let cfg = ColorConfig {
        output_gamut: Gamut::DisplayP3,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert_eq!(f.rgb, vec![210, 8, 4], "sRGB 红应落到 P3 原色坐标");
}

#[test]
fn p3_white_stays_white() {
    let mut f = frame(&[255, 255, 255]);
    let cfg = ColorConfig {
        output_gamut: Gamut::DisplayP3,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert!(f.rgb.iter().all(|&v| v == 255), "{:?}", f.rgb);
}

#[test]
fn gamut_primaries_expose_p3_chromaticities() {
    assert!(Gamut::Srgb.primaries().is_none());
    let primaries = Gamut::DisplayP3.primaries().expect("P3 应有 cHRM 原色");
    assert_eq!(primaries[0], (0.3127, 0.3290));
    assert_eq!(primaries[1], (0.680, 0.320));
    assert_eq!(primaries[2], (0.265, 0.690));
    assert_eq!(primaries[3], (0.150, 0.060));
}
