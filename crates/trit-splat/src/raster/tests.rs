use super::*;
use crate::camera::Intrinsics;
use crate::ply::RawGaussian;

mod border;
mod degenerate;

const SH0_REF: f32 = 0.282_094_8;

/// 纯色 DC 系数：color = f_dc·C0 + 0.5，⇒ f_dc = (target−0.5)/C0。
fn pure_dc(target: [f32; 3]) -> [f32; 3] {
    target.map(|t| (t - 0.5) / SH0_REF)
}

fn gaussian(pos: [f32; 3], color_dc: [f32; 3], scale: f32) -> RawGaussian {
    RawGaussian {
        position: pos,
        f_dc: color_dc,
        f_rest: vec![],
        opacity_logit: 1e8,
        scale_log: [scale.ln(); 3],
        rot_xyzw: [0.0, 0.0, 0.0, 1.0],
    }
}

fn cam_at(z: f32) -> Mat4 {
    let mut m = Mat4::identity();
    m.m[2][3] = z;
    m
}

fn intr_small() -> Intrinsics {
    Intrinsics {
        fl_x: 50.0,
        fl_y: 50.0,
        cx: 16.0,
        cy: 12.0,
        width: 32,
        height: 24,
    }
}

#[test]
fn single_red_gaussian_paints_center() {
    // 高斯在光轴上 (0,0,0)，相机在 z=5：中心像素应为纯红
    let g = gaussian([0.0, 0.0, 0.0], pure_dc([1.0, 0.0, 0.0]), 0.2);
    let frame = render(&[g], &cam_at(5.0), &intr_small(), [0.0, 0.0, 0.0], None);
    let o = (12 * 32 + 16) * 3;
    assert!(frame.rgb[o] > 200, "中心应偏红: {}", frame.rgb[o]);
    assert!(frame.rgb[o + 1] < 40, "中心绿应低: {}", frame.rgb[o + 1]);
    // 角落保持背景
    let corner = 0;
    assert!(frame.rgb[corner] < 5 && frame.rgb[corner + 2] < 5);
}

#[test]
fn nearer_splat_wins_at_overlap() {
    // 同轴两颗：近的绿 (z=3) 压过远的红 (z=0)，相机 z=5
    let far = gaussian([0.0, 0.0, 0.0], pure_dc([1.0, 0.0, 0.0]), 0.25);
    let near = gaussian([0.0, 0.0, 2.0], pure_dc([0.0, 1.0, 0.0]), 0.25);
    let frame = render(
        &[far, near],
        &cam_at(5.0),
        &intr_small(),
        [0.0, 0.0, 0.0],
        None,
    );
    let o = (12 * 32 + 16) * 3;
    assert!(frame.rgb[o + 1] > 200, "近的绿应胜出: {}", frame.rgb[o + 1]);
    assert!(frame.rgb[o] < 40, "远的红应被盖: {}", frame.rgb[o]);
}
