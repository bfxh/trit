//! 白平衡单测：6500K 中性、tint 绿-洋红偏移。

use crate::Frame;
use crate::color::{ColorConfig, WhiteBalance};

fn frame(rgb: &[u8]) -> Frame {
    Frame {
        width: (rgb.len() / 3) as u32,
        height: 1,
        rgb: rgb.to_vec(),
    }
}

#[test]
fn neutral_white_balance_is_close_to_identity() {
    let mut f = frame(&[100, 100, 100]);
    let cfg = ColorConfig {
        white_balance: WhiteBalance {
            temperature_k: 6500.0,
            ..WhiteBalance::default()
        },
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    for c in f.rgb {
        assert!((c as i32 - 100).abs() <= 1, "wb 中性偏差: {c}");
    }
}

#[test]
fn tint_alone_shifts_green() {
    let mut f = frame(&[100, 100, 100]);
    let cfg = ColorConfig {
        white_balance: WhiteBalance {
            tint: 0.5,
            ..WhiteBalance::default()
        },
        ..ColorConfig::default()
    };
    cfg.apply(&mut f);
    assert!(f.rgb[1] > f.rgb[0], "正 tint 应偏绿: {:?}", f.rgb);
    assert!(f.rgb[1] > f.rgb[2], "正 tint 应偏绿: {:?}", f.rgb);
}
