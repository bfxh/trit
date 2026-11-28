use super::*;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

#[test]
fn identity_world_to_cam_is_translation_only() {
    let m = Mat4::identity();
    let p = m.world_to_cam(Vec3::new(1.0, 2.0, 3.0));
    assert!(close(p.x, 1.0) && close(p.y, 2.0) && close(p.z, 3.0));
}

#[test]
fn translation_is_extracted_from_last_column() {
    let mut m = Mat4::identity();
    m.m[0][3] = 4.0;
    m.m[1][3] = -2.0;
    m.m[2][3] = 7.0;
    let t = m.translation();
    assert!(close(t.x, 4.0) && close(t.y, -2.0) && close(t.z, 7.0));
}

#[test]
fn covariance_of_identity_rotation_is_diagonal_scale_squared() {
    let s = covariance([1.0, 0.0, 0.0, 0.0], [2.0, 3.0, 4.0]);
    assert!(close(s[0][0], 4.0) && close(s[1][1], 9.0) && close(s[2][2], 16.0));
    assert!(close(s[0][1], 0.0) && close(s[0][2], 0.0) && close(s[1][2], 0.0));
}

#[test]
fn quat_xyzw_identity_maps_to_unit_rotation() {
    let r = quat_to_mat3([0.0, 0.0, 0.0, 1.0]); // xyzw: w=1 单位旋转
    assert!(close(r[0][0], 1.0) && close(r[1][1], 1.0) && close(r[2][2], 1.0));
}
