//! 高斯泼溅投影：激活 → 3D 协方差 → EWA 屏幕空间 2D 高斯 + SH3 视角色。
//!
//! 数学口径与 3DGS/EWA 一致：`Σ_cam = W Σ_w Wᵀ`，`Σ_2d = J Σ_cam Jᵀ + 0.3·I`
//! （0.3 为像素膨胀，标准做法）；每像素 α = sigmoid(opacity)·exp(−½ dᵀ Σ₂d⁻¹ d)。

use crate::math::{Mat4, Vec3, covariance};
use crate::ply::RawGaussian;

/// SH 视角色用的常量（3DGS 标准系数）。保留参考实现的完整十进制精度，
/// 截断到 f32 可表示位数会改变末位、影响逐像素对拍。
#[allow(clippy::excessive_precision)]
const SH_C0: f32 = 0.282_094_791_773_878_14;
#[allow(clippy::excessive_precision)]
const SH_C1: f32 = 0.488_602_511_902_919_89;
#[allow(clippy::excessive_precision)]
const SH_C2: [f32; 5] = [
    1.092_548_430_592_079_2,
    -1.092_548_430_592_079_2,
    0.315_391_565_252_520_05,
    -1.092_548_430_592_079_2,
    0.546_274_215_296_039_6,
];

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

/// SH 视角色。`f_dc` 即 gaussian-splatting 的 `sh[0]`（0 阶）；`f_rest` 按 Brush ply 的
/// channel-major 布局（f_rest_{c·K + k} = 通道 c 的第 k 个后续系数，K = len/3；
/// SH3 → K=15）。基函数索引与 gaussian-splatting eval_sh 同源：
/// l1: `sh[1..3]`（−y, z, x），l2: `sh[4..8]`，l3: `sh[9..15]`。
/// `view_dir` 从高斯指向相机（归一化）。
pub fn eval_color(f_dc: [f32; 3], f_rest: &[f32], view_dir: Vec3) -> [f32; 3] {
    let mut color = [
        f_dc[0] * SH_C0 + 0.5,
        f_dc[1] * SH_C0 + 0.5,
        f_dc[2] * SH_C0 + 0.5,
    ];
    let (x, y, z) = (view_dir.x, view_dir.y, view_dir.z);
    let k = f_rest.len() / 3; // 每通道系数数（SH1=3, SH2=8, SH3=15）
    if k < 3 {
        return color.map(|v| v.clamp(0.0, 1.0));
    }
    let coeff = |idx: usize, c: usize| f_rest[c * k + idx]; // channel-major（Brush 导出口径）
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, yz, xz) = (x * y, y * z, x * z);
    for (c, channel) in color.iter_mut().enumerate() {
        *channel += SH_C1 * (-y * coeff(0, c) + z * coeff(1, c) - x * coeff(2, c));
        if k >= 8 {
            *channel += SH_C2[0] * xy * coeff(3, c)
                + SH_C2[1] * yz * coeff(4, c)
                + SH_C2[2] * (2.0 * zz - xx - yy) * coeff(5, c)
                + SH_C2[3] * xz * coeff(6, c)
                + SH_C2[4] * (xx - yy) * coeff(7, c);
        }
        if k >= 15 {
            // l3 用 Brush/Sloan(JCGT 2013) 的多项式形式——与 3DGS 教科书版
            // (sh_utils.py) 在 l3 段基函数不同，系数是按 Brush 约定烘的。
            let z2 = zz;
            let f0c = -2.285_229 * z2 + 0.457_045_8;
            let f1b = 1.445_305_7 * z;
            let f2a = -0.590_043_6;
            let fc1 = xx - yy;
            let fs1 = 2.0 * xy;
            let fc2 = x * fc1 - y * fs1;
            let fs2 = x * fs1 + y * fc1;
            let p9 = f2a * fs2;
            let p10 = f1b * fs1;
            let p11 = f0c * y;
            let p12 = z * (1.865_881_7 * z2 - 1.119_529);
            let p13 = f0c * x;
            let p14 = f1b * fc1;
            let p15 = f2a * fc2;
            *channel += p9 * coeff(8, c)
                + p10 * coeff(9, c)
                + p11 * coeff(10, c)
                + p12 * coeff(11, c)
                + p13 * coeff(12, c)
                + p14 * coeff(13, c)
                + p15 * coeff(14, c);
        }
    }
    color.map(|v| v.clamp(0.0, 1.0))
}

/// 把一颗高斯投影成屏幕空间 2D 高斯；投影后方差过小/在相机后 → None。
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

    // J = ∂(u,v)/∂(x_cam,y_cam,z_cam)（OpenGL −Z 前）
    let fl_x = intr.fl_x;
    let fl_y = intr.fl_y;
    // J = ∂(u,v)/∂(x_cam,y_cam,z_cam)。v = cy − fl·y/depth ⇒ ∂v/∂y 与 ∂v/∂z
    // 均为负——y 行符号错了会镜像各向异性 splat 的朝向（cov2d 非对角元翻转）。
    // 注意：Brush 的 pinhole 雅可比虽带 x/z 钳制（视锥外 15% 余量），但实验
    // 证明钳制会破坏背景巨型 splat 的合法贡献（29.17→22.91），已移除。
    // Brush 实际走 RT8 路径（"OPENCV"→RT8），其 lim_r 语义待 v4 单独验证。
    let j = [
        [fl_x / depth, 0.0, fl_x * cam.x / (depth * depth)],
        [0.0, -fl_y / depth, -fl_y * cam.y / (depth * depth)],
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
