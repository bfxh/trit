use crate::math::Vec3;
use crate::splat::eval_color;

#[test]
fn sh1_directional_term_changes_with_view() {
    // channel-major：K=3（SH1 每通道 3 系数），通道 0 的 −y 系数 0.5
    let rest = vec![0.5, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let up = eval_color([0.0; 3], &rest, Vec3::new(0.0, 1.0, 0.0));
    let down = eval_color([0.0; 3], &rest, Vec3::new(0.0, -1.0, 0.0));
    assert!((up[0] - down[0]).abs() > 1e-3);
}
