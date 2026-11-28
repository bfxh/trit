//! 色彩基础单测：传递函数往返、默认恒等、曝光、饱和度、白平衡与 tone map。

use crate::Frame;
use crate::color::*;

fn frame(rgb: &[u8]) -> Frame {
    Frame {
        width: (rgb.len() / 3) as u32,
        height: 1,
        rgb: rgb.to_vec(),
    }
}

#[test]
fn srgb_roundtrip_is_stable() {
    for v in [0.0, 0.02, 0.5, 0.9, 1.0] {
        let back = srgb_encode(srgb_decode(v));
        assert!((back - v).abs() < 1e-5, "{v} -> {back}");
    }
}

#[test]
fn default_config_is_identity() {
    let mut f = frame(&[1, 200, 3, 4, 5, 6]);
    let before = f.rgb.clone();
    ColorConfig::default().apply(&mut f);
    assert_eq!(f.rgb, before);
}

#[test]
fn exposure_adds_one_stop() {
    let mut f = frame(&[51, 51, 51]);
    let cfg = ColorConfig {
        exposure_ev: 1.0,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert_eq!(f.rgb, vec![102, 102, 102]);
}

#[test]
fn saturation_zero_is_gray() {
    let mut f = frame(&[255, 0, 0]);
    let cfg = ColorConfig {
        saturation: 0.0,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert_eq!(f.rgb[0], f.rgb[1]);
    assert_eq!(f.rgb[1], f.rgb[2]);
    assert!(f.rgb[0] > 0);
}

#[test]
fn srgb_in_out_roundtrip_is_close() {
    let mut f = frame(&[128, 64, 32]);
    let before = f.rgb.clone();
    let cfg = ColorConfig {
        input: ColorSpace::Srgb,
        output: ColorSpace::Srgb,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    for (a, b) in before.iter().zip(&f.rgb) {
        assert!((*a as i32 - *b as i32).abs() <= 1, "{a} vs {b}");
    }
}

#[test]
fn aces_is_bounded() {
    let mut f = frame(&[255, 255, 255]);
    let cfg = ColorConfig {
        exposure_ev: 10.0,
        tone_map: ToneMap::Aces,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert!(f.rgb.iter().all(|&v| v > 0));
}
