//! 感知色域映射单测：保亮度、压色度与超色域不再逐通道截断。

use crate::Frame;
use crate::color::ColorConfig;
use crate::color::gamut::map::perceptual;

fn frame(rgb: &[u8]) -> Frame {
    Frame {
        width: (rgb.len() / 3) as u32,
        height: 1,
        rgb: rgb.to_vec(),
    }
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[test]
fn in_gamut_linear_rgb_is_unchanged() {
    let c = [0.4, 0.2, 0.1];
    assert_eq!(perceptual(c), c);
}

#[test]
fn out_of_gamut_keeps_luma_and_fits_cube() {
    let c = [2.0, 0.5, 0.2];
    let out = perceptual(c);
    assert!(out.iter().all(|v| (0.0..=1.0).contains(v)), "{out:?}");
    assert!((luma(out) - luma(c)).abs() < 1e-5, "{out:?}");
    assert!(out[1] > 0.5 && out[2] > 0.2, "色度被保留: {out:?}");
}

#[test]
fn overbright_neutral_maps_to_white() {
    assert_eq!(perceptual([1.2, 1.0, 0.9]), [1.0; 3]);
}

#[test]
fn negative_luma_maps_to_black() {
    assert_eq!(perceptual([-0.1, 0.0, 0.0]), [0.0; 3]);
}

#[test]
fn config_maps_out_of_gamut_saturation_without_channel_clipping() {
    let mut f = frame(&[255, 51, 0]);
    let cfg = ColorConfig {
        saturation: 2.0,
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert_eq!(f.rgb[0], 255);
    assert!(f.rgb[1] > 20, "绿通道应被保留而不是截断: {:?}", f.rgb);
    assert_eq!(f.rgb[2], 0, "蓝通道仍应在边界: {:?}", f.rgb);
}
