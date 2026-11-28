//! 高斯泼溅投影：激活 → 3D 协方差 → EWA 屏幕空间 2D 高斯 + SH3 视角色。
//!
//! 数学口径与 3DGS/EWA 一致：`Σ_cam = W Σ_w Wᵀ`，`Σ_2d = J Σ_cam Jᵀ + 0.3·I`
//! （0.3 为像素膨胀，标准做法）；每像素 α = sigmoid(opacity)·exp(−½ dᵀ Σ₂d⁻¹ d)。

use crate::math::{Mat4, Vec3, covariance};
use crate::ply::RawGaussian;

mod color;

#[cfg(test)]
pub(crate) use color::SH_C0;
pub use color::eval_color;

/// 屏幕空间高斯：中心像素坐标、2D 协方差的逆 (a b; b c)、每像素 α、颜色。
#[derive(Clone, Copy, Debug)]
pub struct ScreenGaussian {
    pub u: f32,
    pub v: f32,
    pub inv_a: f32,
    pub inv_b: f32,
    pub inv_c: f32,
    pub alpha: f32,
    pub depth: f32,
    pub color: [f32; 3],
}

/// `min_scale`：Mip-Splatting 3D 滤波器的每 splat 世界尺度下限（Brush
/// `apply_scale_floor` 语义——floored = √(scale²+f²) 每轴，opacity 按体积比
/// rx·ry·rz 补偿）；None 表示关闭。
#[allow(clippy::too_many_arguments)]
pub fn project(
    g: &RawGaussian,
    c2w: &Mat4,
    intr: &crate::camera::Intrinsics,
    min_scale: Option<f32>,
) -> Option<ScreenGaussian> {
    let (mut scale, rot, mut base_alpha) = crate::ply::activate(g);
    if let Some(f) = min_scale {
        let f2 = f * f;
        let floored = [
            (scale[0] * scale[0] + f2).sqrt(),
            (scale[1] * scale[1] + f2).sqrt(),
            (scale[2] * scale[2] + f2).sqrt(),
        ];
        let comp = (scale[0] / floored[0]) * (scale[1] / floored[1]) * (scale[2] / floored[2]);
        scale = floored;
        base_alpha *= comp;
    }
    let pos = Vec3::new(g.position[0], g.position[1], g.position[2]);
    let cam = c2w.world_to_cam(pos);
    let depth = -cam.z;
    if depth < 0.05 {
        return None; // 相机后方/过近
    }

    // Σ_cam = Wᵀ Σ_w W：world→cam 的线性映射是 Rᵀ（p_cam = Rᵀ(p−t)），
    // 协方差随之用 Rᵀ Σ R——写成 R Σ Rᵀ 会让有俯仰角的相机把各向异性
    // splat 全部转错朝向（实测表现：整幅涂抹感）。
    let sigma_w = covariance(rot, scale);
    let r = c2w.rot3();
    let mut sigma_cam = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let mut acc = 0.0;
            for k in 0..3 {
                for l in 0..3 {
                    acc += r[k][i] * sigma_w[k][l] * r[l][j];
                }
            }
            sigma_cam[i][j] = acc;
        }
    }

    // J = ∂(u,v)/∂(x_cam,y_cam,z_cam)（OpenGL −Z 前；v = cy − fl·y/depth，
    // ∂v/∂y 与 ∂v/∂z 均为负——y 行符号错了会镜像各向异性 splat 的朝向）。
    // 第三列用归一化坐标 x/z、y/z 的 15% 画面外余量截断，与 Brush pinhole
    // `calculate_jacobian_clamp_limits` 一致：视锥边缘的透视投影会发散，
    // 不截断会让贴边巨型 splat 的协方差被放大成整屏涂抹。
    let fl_x = intr.fl_x;
    let fl_y = intr.fl_y;
    let lim_pos_x = (1.15 * intr.width as f32 - intr.cx) / fl_x;
    let lim_neg_x = (-0.15 * intr.width as f32 - intr.cx) / fl_x;
    let lim_pos_y = (1.15 * intr.height as f32 - intr.cy) / fl_y;
    let lim_neg_y = (-0.15 * intr.height as f32 - intr.cy) / fl_y;
    let nx = (cam.x / depth).clamp(lim_neg_x, lim_pos_x);
    let ny = (cam.y / depth).clamp(lim_neg_y, lim_pos_y);
    let j = [
        [fl_x / depth, 0.0, fl_x * nx / depth],
        [0.0, -fl_y / depth, -fl_y * ny / depth],
    ];
    // Σ_2d = J Σ_cam Jᵀ + 0.3·I（0.3px 膨胀，标准做法；实验证伪了 Mip 补偿变体）。
    let jt = [[j[0][0], j[1][0]], [j[0][1], j[1][1]], [j[0][2], j[1][2]]];
    let mut cov2 = [[0.0f32; 2]; 2];
    for i in 0..2 {
        for k in 0..2 {
            let mut acc = 0.0;
            for a in 0..3 {
                for b in 0..3 {
                    acc += j[i][a] * sigma_cam[a][b] * jt[b][k];
                }
            }
            cov2[i][k] = acc + if i == k { 0.3 } else { 0.0 };
        }
    }
    let det = cov2[0][0] * cov2[1][1] - cov2[0][1] * cov2[1][0];
    if det <= 1e-12 {
        return None;
    }
    let inv_a = cov2[1][1] / det;
    let inv_b = -cov2[0][1] / det;
    let inv_c = cov2[0][0] / det;

    let u = fl_x * (cam.x / depth) + intr.cx;
    let v = intr.cy - fl_y * (cam.y / depth); // +Y 朝上 vs 图像 v 朝下
    // 视方向口径与 gaussian-splatting eval_sh 一致：从相机指向高斯（ray 方向）。
    // 奇数阶 l1/l3 依赖此方向——方向反了全部变号（实测差 0.6 dB 以上且物体区噪声）。
    let dir = Vec3::new(
        pos.x - c2w.translation().x,
        pos.y - c2w.translation().y,
        pos.z - c2w.translation().z,
    )
    .normalized();
    let color = eval_color(g.f_dc, &g.f_rest, dir);
    Some(ScreenGaussian {
        u,
        v,
        inv_a,
        inv_b,
        inv_c,
        alpha: base_alpha,
        depth,
        color,
    })
}

#[cfg(test)]
mod tests;
