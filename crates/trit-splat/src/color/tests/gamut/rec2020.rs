//! Rec.2020 原色转换与 cHRM 原色坐标。

use crate::Frame;
use crate::color::{ColorConfig, Gamut, bt1886_decode, bt1886_encode};

fn frame(rgb: &[u8]) -> Frame {
    Frame {
        width: (rgb.len() / 3) as u32,
        height: 1,
        rgb: rgb.to_vec(),
    }
}

#[test]
fn bt1886_roundtrip_is_stable() {
    for v in [0.0, 0.02, 0.5, 0.9, 1.0] {
        let back = bt1886_encode(bt1886_decode(v));
        assert!((back - v).abs() < 1e-5, "{v} -> {back}");
    }
}

#[test]
fn rec2020_red_is_converted_from_srgb() {
    let mut f = frame(&[255, 0, 0]);
    let cfg = ColorConfig {
        output_gamut: Gamut::Rec2020,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert_eq!(f.rgb, vec![160, 18, 4], "sRGB 红应落到 Rec.2020 原色坐标");
}

#[test]
fn rec2020_white_stays_white() {
    let mut f = frame(&[255, 255, 255]);
    let cfg = ColorConfig {
        output_gamut: Gamut::Rec2020,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert!(f.rgb.iter().all(|&v| v == 255), "{:?}", f.rgb);
}

#[test]
fn rec2020_primaries_expose_bt2020_chromaticities() {
    let primaries = Gamut::Rec2020.primaries().expect("Rec.2020 应有 cHRM 原色");
    assert_eq!(primaries[0], (0.3127, 0.3290));
    assert_eq!(primaries[1], (0.708, 0.292));
    assert_eq!(primaries[2], (0.170, 0.797));
    assert_eq!(primaries[3], (0.131, 0.046));
}
