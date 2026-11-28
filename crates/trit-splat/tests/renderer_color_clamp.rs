//! SH 视角色只做下界钳制：负辐亮度归零，HDR 高光保留到最终落盘。

use trit_splat::math::Vec3;
use trit_splat::splat::eval_color;

#[test]
fn negative_sh_color_is_clamped_to_zero() {
    // channel-major K=3；通道 0 的 −y 系数 10，视线 +y ⇒ 0.5 − 4.886 < 0
    let rest = vec![10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let c = eval_color([0.0; 3], &rest, Vec3::new(0.0, 1.0, 0.0));
    assert_eq!(c[0], 0.0);
}

#[test]
fn hdr_sh_highlight_is_not_clamped_per_splat() {
    // 同一系数反向视线 ⇒ 0.5 + 4.886 > 1，应保留给 alpha 合成
    let rest = vec![10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    let c = eval_color([0.0; 3], &rest, Vec3::new(0.0, -1.0, 0.0));
    assert!(c[0] > 1.0, "HDR 高光被逐 splat 截断: {}", c[0]);
}
