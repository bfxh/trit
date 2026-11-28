//! SH 视角色：DC + l1/l2/l3，按 Brush/Sloan 基函数求值。
//!
//! 颜色在每 splat 上只做 **下界钳制**（负辐亮度无物理意义），不设上界：
//! 高光 splat 的 SH 分量可以超过 1，若逐 splat 截到 [0,1]，alpha 合成前
//! 就丢掉 HDR 能量，亮面/镜面边缘会系统性变暗。上界只在最终 RGB8
//! 落盘时钳一次（与 Brush rasterizer 的 `max(color, 0)` 口径一致）。

use crate::math::Vec3;

/// SH 视角色用的常量（3DGS 标准系数）。保留参考实现的完整十进制精度，
/// 截断到 f32 可表示位数会改变末位、影响逐像素对拍。
#[allow(clippy::excessive_precision)]
pub(crate) const SH_C0: f32 = 0.282_094_791_773_878_14;
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
        return color.map(|v| v.max(0.0));
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
    color.map(|v| v.max(0.0))
}
