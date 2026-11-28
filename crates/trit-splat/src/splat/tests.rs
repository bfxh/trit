use super::*;

fn dummy() -> RawGaussian {
    RawGaussian {
        position: [0.0, 0.0, 0.0], // 相机在 (0,0,5) 看 −Z：正前方 5m、在光轴上
        f_dc: [0.0; 3],
        f_rest: vec![],
        opacity_logit: 1e8, // sigmoid → 1
        scale_log: [0.0, 0.0, 0.0].map(f32::ln),
        rot_xyzw: [0.0, 0.0, 0.0, 1.0],
    }
}

fn identity_cam_at_z5() -> Mat4 {
    // 相机在 (0,0,5) 看向原点：c2w = T(0,0,5)
    let mut m = Mat4::identity();
    m.m[0][3] = 0.0;
    m.m[1][3] = 0.0;
    m.m[2][3] = 5.0;
    m
}

fn intr() -> crate::camera::Intrinsics {
    crate::camera::Intrinsics { fl_x: 1120.0, fl_y: 1120.0, cx: 480.0, cy: 270.0, width: 960, height: 540 }
}

#[test]
fn sh0_dc_only_color_is_c0_plus_half() {
    let c = eval_color([0.35, 0.5, 0.65], &[], Vec3::new(0.0, 0.0, 1.0));
    assert!((c[0] - (0.35 * SH_C0 + 0.5)).abs() < 1e-5);
    assert!((c[2] - (0.65 * SH_C0 + 0.5)).abs() < 1e-5);
}

#[test]
fn sh1_directional_term_changes_with_view() {
    // channel-major：K=3（SH1 每通道 3 系数），通道 0 的 −y 系数 0.5
    let rest = vec![0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let up = eval_color([0.0; 3], &rest, Vec3::new(0.0, 1.0, 0.0));
    let down = eval_color([0.0; 3], &rest, Vec3::new(0.0, -1.0, 0.0));
    assert!((up[0] - down[0]).abs() > 1e-3);
}

#[test]
fn on_axis_gaussian_centers_on_principal_point() {
    let g = dummy();
    let s = project(&g, &identity_cam_at_z5(), &intr(), None).unwrap();
    assert!((s.u - 480.0).abs() < 1e-3);
    assert!((s.v - 270.0).abs() < 1e-3);
    assert!((s.depth - 5.0).abs() < 1e-4);
    assert!((s.alpha - 1.0).abs() < 1e-3);
}

#[test]
fn gaussian_behind_camera_is_culled() {
    let mut g = dummy();
    g.position = [0.0, 0.0, 9.0]; // 相机在 z=5，后方
    assert!(project(&g, &identity_cam_at_z5(), &intr(), None).is_none());
}

#[test]
fn covariance_inverse_is_positive_definite() {
    let g = dummy();
    let s = project(&g, &identity_cam_at_z5(), &intr(), None).unwrap();
    let det = 1.0 / (s.inv_a * s.inv_c - s.inv_b * s.inv_b);
    assert!(det > 0.0);
    assert!(s.inv_a > 0.0 && s.inv_c > 0.0);
}
