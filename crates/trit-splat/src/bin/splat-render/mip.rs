//! Mip-Splatting 3D 滤波的每 splat 世界尺度下限。

use trit_splat::camera::Transforms;
use trit_splat::math::Vec3;
use trit_splat::ply::RawGaussian;

/// `f_i = √factor · min_v(dist/focal)`（Brush train.rs:98 口径）。
/// factor ≤ 0 表示关闭，返回 None。
pub fn min_scales(
    gaussians: &[RawGaussian],
    transforms: &Transforms,
    factor: f32,
) -> Option<Vec<f32>> {
    if factor <= 0.0 {
        return None;
    }
    let fl = transforms.intrinsics.fl_x;
    let mut f = vec![0.0f32; gaussians.len()];
    for (idx, g) in gaussians.iter().enumerate() {
        let mu = Vec3::new(g.position[0], g.position[1], g.position[2]);
        let mut min_ratio = f32::INFINITY;
        for frame in &transforms.frames {
            let c = frame.c2w.translation();
            let d = mu.subtract(c).length();
            min_ratio = min_ratio.min(d / fl);
        }
        f[idx] = factor.sqrt() * min_ratio;
    }
    Some(f)
}
