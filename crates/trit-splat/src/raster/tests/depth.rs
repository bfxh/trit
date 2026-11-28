//! 深度缓冲：单高斯视深、近处遮蔽、背景 +∞。

use crate::camera::Intrinsics;
use crate::math::Mat4;
use crate::ply::RawGaussian;
use crate::raster::render_with_depth;

const SH0_REF: f32 = 0.282_094_8;

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
fn single_gaussian_depth_is_camera_distance() {
    let g = gaussian([0.0, 0.0, 0.0], pure_dc([1.0, 0.0, 0.0]), 0.2);
    let (_, depth) = render_with_depth(&[g], &cam_at(5.0), &intr_small(), [0.0; 3], None);
    assert_eq!(depth.len(), 32 * 24);
    let center = depth[12 * 32 + 16];
    assert!((center - 5.0).abs() < 1e-3, "中心视深应 ≈5: {center}");
    assert!(depth[0].is_infinite(), "角落无覆盖应 +∞: {}", depth[0]);
}

#[test]
fn nearer_opaque_splat_wins_depth() {
    // 同轴：远的红在 z=0（视深 5），近的绿在 z=2（视深 3）。
    let far = gaussian([0.0, 0.0, 0.0], pure_dc([1.0, 0.0, 0.0]), 0.25);
    let near = gaussian([0.0, 0.0, 2.0], pure_dc([0.0, 1.0, 0.0]), 0.25);
    let (_, depth) = render_with_depth(&[far, near], &cam_at(5.0), &intr_small(), [0.0; 3], None);
    let center = depth[12 * 32 + 16];
    assert!(
        (center - 3.0).abs() < 0.1,
        "近处 opaque 应主导深度: {center}"
    );
}
