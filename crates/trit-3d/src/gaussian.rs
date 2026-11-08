#![allow(clippy::needless_range_loop)]
//! 完整 3DGS 头：latent → N 个高斯（位置 / 尺度 / 旋转四元数 / 不透明度 / SH 颜色）。
//!
//! 参数布局（每个高斯）：
//! `pos(3) + scale(3) + quat(4) + opacity(1) + SH(coeffs × 3)`。
//! SH 阶数 0/1/2/3 分别有 1/4/9/16 个基函数，对应每高斯 **14 / 23 / 38 / 59** 个参数。
//!
//! 颜色按 3DGS 的 real-SH 约定求值：`C(d) = clamp(0.5 + Σ c_i·Y_i(d), 0, 1)`。
//! 训练同时监督参数 MSE 与 **多视角颜色**（8 个方向），因此高阶 SH 有非零梯度；
//! 目标高斯的 l=1/l=2/l=3 系数按固定方向函数设置（不是全零），可验证高阶确实被学出来。
//!
//! 三值位置同 `lib.rs`：全连接权重三值，参数回归头 f32（B 类模拟税）。

use trit_learn::{Activation, Adam, Ste, TernaryDense};

use crate::{DenseEncoder, VoxelGrid};

/// 球谐阶数。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShDegree {
    /// 0 阶：只有 DC（1 个基函数）。
    Dc,
    /// 1 阶：4 个基函数。
    One,
    /// 2 阶：9 个基函数。
    Two,
    /// 3 阶：16 个基函数。
    Three,
}

impl ShDegree {
    /// 每通道的基函数个数。
    pub fn coeffs(self) -> usize {
        match self {
            Self::Dc => 1,
            Self::One => 4,
            Self::Two => 9,
            Self::Three => 16,
        }
    }

    /// 每个高斯的参数个数：`pos3 + scale3 + quat4 + opacity1 + coeffs×3`。
    pub fn params(self) -> usize {
        11 + self.coeffs() * 3
    }
}

/// 3DGS / real-SH 基函数（前 `coeffs` 个，方向会被归一化）。
pub fn sh_basis(degree: ShDegree, dir: [f32; 3]) -> Vec<f32> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    let (x, y, z) = if len > 1e-8 {
        (dir[0] / len, dir[1] / len, dir[2] / len)
    } else {
        (0.0, 0.0, 1.0)
    };
    let mut y0 = vec![0.0f32; 16];
    y0[0] = 0.282_094_8;
    y0[1] = -0.488_602_5 * y;
    y0[2] = 0.488_602_5 * z;
    y0[3] = -0.488_602_5 * x;
    y0[4] = 1.092_548_4 * x * y;
    y0[5] = -1.092_548_4 * y * z;
    y0[6] = 0.315_391_57 * (2.0 * z * z - x * x - y * y);
    y0[7] = -1.092_548_4 * x * z;
    y0[8] = 0.546_274_2 * (x * x - y * y);
    y0[9] = 0.590_043_56 * y * (3.0 * x * x - y * y);
    y0[10] = 2.890_611_4 * x * y * z;
    y0[11] = 0.457_045_8 * y * (5.0 * z * z - 1.0);
    y0[12] = 0.373_176_34 * z * (5.0 * z * z - 3.0);
    y0[13] = 0.457_045_8 * x * (5.0 * z * z - 1.0);
    y0[14] = 1.445_305_7 * z * (x * x - y * y);
    y0[15] = 0.590_043_56 * x * (x * x - 3.0 * y * y);
    y0.truncate(degree.coeffs());
    y0
}

/// 由 SH 系数求给定方向的 RGB。
pub fn sh_color(sh: &[f32], degree: ShDegree, dir: [f32; 3]) -> [f32; 3] {
    let basis = sh_basis(degree, dir);
    let mut rgb = [0.5f32; 3];
    for (i, &b) in basis.iter().enumerate() {
        for c in 0..3 {
            rgb[c] += sh[3 * i + c] * b;
        }
    }
    [
        rgb[0].clamp(0.0, 1.0),
        rgb[1].clamp(0.0, 1.0),
        rgb[2].clamp(0.0, 1.0),
    ]
}

/// 一个完整 3DGS 高斯。
#[derive(Clone, Debug, PartialEq)]
pub struct Gaussian {
    /// 归一化世界坐标，`[-1,1]³`。
    pub pos: [f32; 3],
    /// 各轴尺度（正数）。
    pub scale: [f32; 3],
    /// 单位四元数 `[w, x, y, z]`。
    pub rotation: [f32; 4],
    /// 不透明度 `(0,1)`。
    pub opacity: f32,
    /// SH 系数：`coeffs × 3`，按“系数优先、RGB 交错”排。
    pub sh: Vec<f32>,
}

impl Gaussian {
    /// 由 `sh.len()/3` 推断阶数。
    pub fn degree(&self) -> ShDegree {
        match self.sh.len() / 3 {
            1 => ShDegree::Dc,
            4 => ShDegree::One,
            9 => ShDegree::Two,
            16 => ShDegree::Three,
            n => panic!("SH 系数个数 {n} 不是 1/4/9/16"),
        }
    }

    /// 给定观察方向的 RGB（3DGS real-SH）。
    pub fn color_at(&self, dir: [f32; 3]) -> [f32; 3] {
        sh_color(&self.sh, self.degree(), dir)
    }

    /// DC 颜色（SH 第 0 项）映射到 `(0,1)`，用于快速预览。
    pub fn color(&self) -> [f32; 3] {
        [
            (0.5 + self.sh[0]).clamp(0.0, 1.0),
            (0.5 + self.sh[1]).clamp(0.0, 1.0),
            (0.5 + self.sh[2]).clamp(0.0, 1.0),
        ]
    }
}

/// 高斯头：`TernaryDense(latent → N×params)` + 逐参数激活。
pub struct GaussianHead {
    dense: TernaryDense,
    n: usize,
    degree: ShDegree,
    out: Vec<f32>,
    quat_norm: Vec<f32>,
}

impl GaussianHead {
    /// 默认 0 阶 SH（每高斯 14 参数）。
    pub fn new(latent: usize, n: usize, seed: u64) -> Self {
        Self::with_degree(latent, n, ShDegree::Dc, seed)
    }

    /// 指定 SH 阶数。
    pub fn with_degree(latent: usize, n: usize, degree: ShDegree, seed: u64) -> Self {
        let init = crate::xorshift_init(seed ^ 0x3C3C_C3C3);
        let params = degree.params();
        Self {
            dense: TernaryDense::new(n * params, latent, init, |_| 0.25, Ste::Identity),
            n,
            degree,
            out: Vec::new(),
            quat_norm: Vec::new(),
        }
    }

    /// 高斯个数。
    pub fn n(&self) -> usize {
        self.n
    }

    /// SH 阶数。
    pub fn degree(&self) -> ShDegree {
        self.degree
    }

    /// 每高斯参数个数。
    pub fn params_per_gaussian(&self) -> usize {
        self.degree.params()
    }

    /// 输出维数。
    pub fn out_dim(&self) -> usize {
        self.n * self.params_per_gaussian()
    }

    /// 前向：返回 `N×params` 扁平参数。
    pub fn forward(&mut self, latent: &[f32]) -> Vec<f32> {
        let pre = self.dense.forward(latent);
        let pp = self.params_per_gaussian();
        let sh = self.degree.coeffs() * 3;
        let mut out = vec![0.0f32; self.n * pp];
        self.quat_norm.clear();
        self.quat_norm.reserve(self.n);
        for i in 0..self.n {
            let b = i * pp;
            for k in 0..3 {
                out[b + k] = Activation::Tanh.forward(pre[b + k]);
            }
            for k in 0..3 {
                out[b + 3 + k] = 0.001 + 0.2 * Activation::Sigmoid.forward(pre[b + 3 + k]);
            }
            let q = [pre[b + 6], pre[b + 7], pre[b + 8], pre[b + 9]];
            let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3])
                .sqrt()
                .max(1e-6);
            self.quat_norm.push(norm);
            for k in 0..4 {
                out[b + 6 + k] = q[k] / norm;
            }
            out[b + 10] = Activation::Sigmoid.forward(pre[b + 10]);
            for k in 0..sh {
                out[b + 11 + k] = 0.5 * Activation::Tanh.forward(pre[b + 11 + k]);
            }
        }
        self.out = out.clone();
        out
    }

    /// 反向：返回对 latent 的梯度。
    pub fn backward(&mut self, grad_out: &[f32]) -> Vec<f32> {
        assert_eq!(grad_out.len(), self.out.len(), "grad_out 维度不对");
        let pp = self.params_per_gaussian();
        let sh = self.degree.coeffs() * 3;
        let mut g = vec![0.0f32; self.out.len()];
        for i in 0..self.n {
            let b = i * pp;
            for k in 0..3 {
                let y = self.out[b + k];
                g[b + k] = grad_out[b + k] * (1.0 - y * y);
            }
            for k in 0..3 {
                let y = self.out[b + 3 + k];
                let s = (y - 0.001) / 0.2;
                g[b + 3 + k] = grad_out[b + 3 + k] * 0.2 * s * (1.0 - s);
            }
            // 四元数归一化的反向：d(q/|q|)/dq = (I - qn·qnᵀ)/|q|。
            let qn = [
                self.out[b + 6],
                self.out[b + 7],
                self.out[b + 8],
                self.out[b + 9],
            ];
            let gn = [
                grad_out[b + 6],
                grad_out[b + 7],
                grad_out[b + 8],
                grad_out[b + 9],
            ];
            let dot = qn[0] * gn[0] + qn[1] * gn[1] + qn[2] * gn[2] + qn[3] * gn[3];
            let inv = 1.0 / self.quat_norm[i];
            for k in 0..4 {
                g[b + 6 + k] = (gn[k] - qn[k] * dot) * inv;
            }
            let o = self.out[b + 10];
            g[b + 10] = grad_out[b + 10] * o * (1.0 - o);
            for k in 0..sh {
                let y = self.out[b + 11 + k];
                let t = y / 0.5;
                g[b + 11 + k] = grad_out[b + 11 + k] * 0.5 * (1.0 - t * t);
            }
        }
        self.dense.backward(&g)
    }

    /// 清零累积梯度。
    pub fn zero_grad(&mut self) {
        self.dense.zero_grad();
    }

    /// 应用累积梯度。
    pub fn update(
        &mut self,
        latent_opt: &mut dyn trit_learn::Optimizer,
        scale_opt: &mut dyn trit_learn::Optimizer,
    ) {
        self.dense.update(latent_opt, scale_opt);
    }
}

/// 参数扁平数组 → 高斯列表。
pub fn to_gaussians(params: &[f32], degree: ShDegree) -> Vec<Gaussian> {
    let pp = degree.params();
    assert!(
        params.len().is_multiple_of(pp),
        "参数长度必须是每高斯参数数的倍数"
    );
    let sh = degree.coeffs() * 3;
    params
        .chunks_exact(pp)
        .map(|p| Gaussian {
            pos: [p[0], p[1], p[2]],
            scale: [p[3], p[4], p[5]],
            rotation: [p[6], p[7], p[8], p[9]],
            opacity: p[10],
            sh: p[11..11 + sh].to_vec(),
        })
        .collect()
}

/// 高斯列表 → 扁平参数（与 [`GaussianHead::forward`] 同布局）。
pub fn flatten_gaussians(gaussians: &[Gaussian], degree: ShDegree) -> Vec<f32> {
    let mut out = Vec::with_capacity(gaussians.len() * degree.params());
    for g in gaussians {
        out.extend_from_slice(&g.pos);
        out.extend_from_slice(&g.scale);
        out.extend_from_slice(&g.rotation);
        out.push(g.opacity);
        out.extend_from_slice(&g.sh);
    }
    out
}

/// 体素表面采样（默认 0 阶 SH）。
pub fn surface_targets(grid: &VoxelGrid, n: usize) -> Vec<Gaussian> {
    surface_targets_sh(grid, n, ShDegree::Dc)
}

/// 目标 SH 系数：固定方向函数，保证 l=1/l=2/l=3 都有非零监督。
///
/// 颜色函数（相对 0.5）：
/// `0.2·x + 0.2·y + 0.2·z + 0.04·x·y + 0.05·(x²-y²) + 0.03·x·y·z`
/// 按通道分配到 3DGS 的 SH 基上；低阶目标在更高阶请求时截断。
pub(crate) fn target_sh(degree: ShDegree) -> Vec<f32> {
    let mut sh = vec![0.0f32; degree.coeffs() * 3];
    sh[0] = 0.1;
    sh[1] = 0.1;
    sh[2] = 0.1;
    // l=1：R←x（Y3），G←y（Y1），B←z（Y2）。
    if sh.len() >= 12 {
        sh[3 * 3] = -0.409_328; // 0.2 / -0.4886
        sh[4] = -0.409_328;
        sh[3 * 2 + 2] = 0.409_328;
    }
    // l=2：G←x·y（Y4），R←x²-y²（Y8）。
    if sh.len() >= 27 {
        sh[3 * 4 + 1] = 0.036_612; // 0.04 / 1.0925
        sh[3 * 8] = 0.091_530; // 0.05 / 0.5463
    }
    // l=3：B←x·y·z（Y10）。
    if sh.len() >= 48 {
        sh[3 * 10 + 2] = 0.010_378; // 0.03 / 2.8906
    }
    sh
}

/// 体素表面采样：内部 `+1` 且六邻域有 `<=0`（或边界）的体素。
///
/// 返回 `n` 个高斯；表面体素不足时循环复用（确定性）。
pub fn surface_targets_sh(grid: &VoxelGrid, n: usize, degree: ShDegree) -> Vec<Gaussian> {
    let s = grid.size;
    let mut surface: Vec<(usize, usize, usize)> = Vec::new();
    for z in 0..s {
        for y in 0..s {
            for x in 0..s {
                if grid.get(x, y, z) <= 0 {
                    continue;
                }
                let boundary = x == 0 || y == 0 || z == 0 || x == s - 1 || y == s - 1 || z == s - 1;
                let open = [
                    (1, 0, 0),
                    (-1, 0, 0),
                    (0, 1, 0),
                    (0, -1, 0),
                    (0, 0, 1),
                    (0, 0, -1),
                ]
                .iter()
                .any(|&(dx, dy, dz)| {
                    let (nx, ny, nz) = (x as isize + dx, y as isize + dy, z as isize + dz);
                    nx < 0
                        || ny < 0
                        || nz < 0
                        || nx >= s as isize
                        || ny >= s as isize
                        || nz >= s as isize
                        || grid.get(nx as usize, ny as usize, nz as usize) <= 0
                });
                if boundary || open {
                    surface.push((x, y, z));
                }
            }
        }
    }
    assert!(!surface.is_empty(), "网格没有表面体素");
    let denom = (s - 1).max(1) as f32;
    let scale = 1.0 / denom;
    let sh = target_sh(degree);
    (0..n)
        .map(|i| {
            let (x, y, z) = surface[i % surface.len()];
            Gaussian {
                pos: [
                    x as f32 / denom * 2.0 - 1.0,
                    y as f32 / denom * 2.0 - 1.0,
                    z as f32 / denom * 2.0 - 1.0,
                ],
                scale: [scale; 3],
                rotation: [1.0, 0.0, 0.0, 0.0],
                opacity: 1.0,
                sh: sh.clone(),
            }
        })
        .collect()
}

/// 多视角颜色监督：返回 `(color_mse, 对 SH 参数的梯度)`。
fn color_loss(
    params: &[f32],
    targets: &[Gaussian],
    degree: ShDegree,
    views: &[[f32; 3]],
) -> (f32, Vec<f32>) {
    let pp = degree.params();
    let coeffs = degree.coeffs();
    let sh = coeffs * 3;
    let mut grad = vec![0.0f32; params.len()];
    let mut loss = 0.0f32;
    let mut count = 0usize;
    for (i, target) in targets.iter().enumerate() {
        let base = i * pp + 11;
        let pred = &params[base..base + sh];
        for view in views {
            let basis = sh_basis(degree, *view);
            let mut rgb = [0.5f32; 3];
            for (k, &b) in basis.iter().enumerate() {
                for c in 0..3 {
                    rgb[c] += pred[3 * k + c] * b;
                }
            }
            let want = target.color_at(*view);
            for c in 0..3 {
                let d = rgb[c] - want[c];
                loss += d * d;
                for (k, &b) in basis.iter().enumerate() {
                    grad[base + 3 * k + c] += 2.0 * d * b;
                }
                count += 1;
            }
        }
    }
    if count > 0 {
        let inv = 1.0 / count as f32;
        loss *= inv;
        for g in &mut grad {
            *g *= inv;
        }
    }
    (loss, grad)
}

/// 8 个观察方向（轴对齐 + 两条斜向）。
pub const VIEWS: [[f32; 3]; 8] = [
    [1.0, 0.0, 0.0],
    [-1.0, 0.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, 1.0],
    [0.0, 0.0, -1.0],
    [0.577, 0.577, 0.577],
    [-0.577, 0.577, -0.577],
];

/// 双向 Chamfer 平方距离（位置部分）。
pub fn chamfer_sq(a: &[Gaussian], b: &[Gaussian]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let one = |from: &[Gaussian], to: &[Gaussian]| {
        from.iter()
            .map(|p| {
                to.iter()
                    .map(|q| {
                        let d = [
                            p.pos[0] - q.pos[0],
                            p.pos[1] - q.pos[1],
                            p.pos[2] - q.pos[2],
                        ];
                        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
                    })
                    .fold(f32::INFINITY, f32::min)
            })
            .sum::<f32>()
            / from.len() as f32
    };
    (one(a, b) + one(b, a)) * 0.5
}

/// 训练报告。
#[derive(Clone, Debug, PartialEq)]
pub struct GaussianReport {
    /// 首步参数 MSE。
    pub first_loss: f32,
    /// 末步参数 MSE。
    pub last_loss: f32,
    /// 首步多视角颜色 MSE。
    pub first_color: f32,
    /// 末步多视角颜色 MSE。
    pub last_color: f32,
    /// 首步 Chamfer²。
    pub first_chamfer: f32,
    /// 末步 Chamfer²。
    pub last_chamfer: f32,
    /// 高斯个数。
    pub n: usize,
    /// 每高斯参数个数。
    pub params: usize,
    /// SH 阶数。
    pub degree: ShDegree,
}

/// 训练稠密编码器 + 高斯头（M0 单样本）。
///
/// 损失 = 参数 MSE + `COLOR_WEIGHT ×` 多视角颜色 MSE。
pub fn train_gaussian_head(
    encoder: &mut DenseEncoder,
    head: &mut GaussianHead,
    grid: &VoxelGrid,
    steps: usize,
    lr: f32,
) -> GaussianReport {
    let targets = surface_targets_sh(grid, head.n(), head.degree());
    train_gaussian_head_with_targets(encoder, head, grid, &targets, steps, lr)
}

/// 用给定的高斯目标训练（真实 mesh 采样时走这条）。
pub fn train_gaussian_head_with_targets(
    encoder: &mut DenseEncoder,
    head: &mut GaussianHead,
    grid: &VoxelGrid,
    targets: &[Gaussian],
    steps: usize,
    lr: f32,
) -> GaussianReport {
    /// 颜色损失权重（颜色量级比参数 MSE 小一个数量级）。
    const COLOR_WEIGHT: f32 = 10.0;

    let degree = head.degree();
    let target_params = flatten_gaussians(targets, degree);

    let mut l1_latent = Adam::new(lr);
    let mut l1_scale = Adam::new(lr);
    let mut l2_latent = Adam::new(lr);
    let mut l2_scale = Adam::new(lr);
    let mut h_latent = Adam::new(lr);
    let mut h_scale = Adam::new(lr);

    let mut first_loss = f32::NAN;
    let mut last_loss = f32::NAN;
    let mut first_color = f32::NAN;
    let mut last_color = f32::NAN;
    let mut first_chamfer = f32::NAN;
    let mut last_chamfer = f32::NAN;
    for _ in 0..steps {
        encoder.zero_grad();
        head.zero_grad();
        let latent = encoder.encode(grid);
        let out = head.forward(&latent);
        let (loss, mut grad) = trit_learn::loss::mse(&out, &target_params);
        let (closs, cgrad) = color_loss(&out, targets, degree, &VIEWS);
        for (g, cg) in grad.iter_mut().zip(&cgrad) {
            *g += cg * COLOR_WEIGHT;
        }
        let chamfer = chamfer_sq(&to_gaussians(&out, degree), targets);
        if first_loss.is_nan() {
            first_loss = loss;
            first_color = closs;
            first_chamfer = chamfer;
        }
        last_loss = loss;
        last_color = closs;
        last_chamfer = chamfer;
        let grad_latent = head.backward(&grad);
        encoder.backward(&grad_latent);
        encoder.update(&mut l1_latent, &mut l1_scale, &mut l2_latent, &mut l2_scale);
        head.update(&mut h_latent, &mut h_scale);
    }
    GaussianReport {
        first_loss,
        last_loss,
        first_color,
        last_color,
        first_chamfer,
        last_chamfer,
        n: head.n(),
        params: head.params_per_gaussian(),
        degree: head.degree(),
    }
}

/// 写标准 3DGS ASCII PLY：`scale_*` 为 log 标准差、`opacity` 为 logit、
/// `rot_0..3` 为 `[w,x,y,z]`；只写 `f_dc_0..2`（M0 不导出高阶 SH）。
pub fn write_ply(gaussians: &[Gaussian], path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(f, "ply")?;
    writeln!(f, "format ascii 1.0")?;
    writeln!(f, "element vertex {}", gaussians.len())?;
    for name in [
        "x", "y", "z", "nx", "ny", "nz", "f_dc_0", "f_dc_1", "f_dc_2", "opacity", "scale_0",
        "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3",
    ] {
        writeln!(f, "property float {name}")?;
    }
    writeln!(f, "end_header")?;
    for g in gaussians {
        let scale = [
            g.scale[0].max(1e-6).ln(),
            g.scale[1].max(1e-6).ln(),
            g.scale[2].max(1e-6).ln(),
        ];
        let op = g.opacity.clamp(1e-4, 1.0 - 1e-4);
        let op_logit = (op / (1.0 - op)).ln();
        let n = quat_axis_z(g.rotation);
        writeln!(
            f,
            "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
            g.pos[0],
            g.pos[1],
            g.pos[2],
            n[0],
            n[1],
            n[2],
            g.sh[0],
            g.sh[1],
            g.sh[2],
            op_logit,
            scale[0],
            scale[1],
            scale[2],
            g.rotation[0],
            g.rotation[1],
            g.rotation[2],
            g.rotation[3]
        )?;
    }
    f.flush()
}
fn quat_axis_z(q: [f32; 4]) -> [f32; 3] {
    let [w, x, y, z] = q;
    [
        2.0 * (x * z + w * y),
        2.0 * (y * z - w * x),
        1.0 - 2.0 * (x * x + y * y),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_basis_reference_values() {
        let y = sh_basis(ShDegree::Dc, [1.0, 0.0, 0.0]);
        assert_eq!(y.len(), 1);
        assert!((y[0] - 0.282_094_8).abs() < 1e-5);
        let y = sh_basis(ShDegree::One, [1.0, 0.0, 0.0]);
        assert_eq!(y.len(), 4);
        assert!((y[3] + 0.488_602_5).abs() < 1e-5, "Y3(+x) 应为 -0.4886");
        let y = sh_basis(ShDegree::Three, [0.0, 0.0, 1.0]);
        assert_eq!(y.len(), 16);
        assert!((y[2] - 0.488_602_5).abs() < 1e-5);
    }

    #[test]
    fn color_varies_with_direction() {
        let g = surface_targets_sh(&VoxelGrid::cube(8, 2), 1, ShDegree::One)[0].clone();
        let a = g.color_at([1.0, 0.0, 0.0]);
        let b = g.color_at([-1.0, 0.0, 0.0]);
        assert!(
            (a[0] - b[0]).abs() > 0.1,
            "R 通道应随 ±x 变化: {a:?} vs {b:?}"
        );
    }

    #[test]
    fn head_shape_and_determinism() {
        let latent = vec![0.3f32, -0.2, 0.1, 0.4];
        let mut a = GaussianHead::new(4, 8, 9);
        let mut b = GaussianHead::new(4, 8, 9);
        let pa = a.forward(&latent);
        let pb = b.forward(&latent);
        assert_eq!(pa.len(), 8 * 14);
        assert_eq!(pa, pb);
        assert!(pa.iter().all(|v| v.is_finite()));
        let gs = to_gaussians(&pa, ShDegree::Dc);
        assert_eq!(gs.len(), 8);
        for g in &gs {
            assert!((0.001..=0.201).contains(&g.scale[0]));
            assert!((0.0..=1.0).contains(&g.opacity));
            let n = (g.rotation[0] * g.rotation[0]
                + g.rotation[1] * g.rotation[1]
                + g.rotation[2] * g.rotation[2]
                + g.rotation[3] * g.rotation[3])
                .sqrt();
            assert!((n - 1.0).abs() < 1e-4, "四元数不是单位长度: {n}");
            assert_eq!(g.sh.len(), 3);
        }
    }

    #[test]
    fn sh_degree_param_counts() {
        assert_eq!(ShDegree::Dc.params(), 14);
        assert_eq!(ShDegree::One.params(), 23);
        assert_eq!(ShDegree::Two.params(), 38);
        assert_eq!(ShDegree::Three.params(), 59);
        let mut h = GaussianHead::with_degree(4, 2, ShDegree::Two, 3);
        let out = h.forward(&[0.1, 0.2, 0.3, 0.4]);
        assert_eq!(out.len(), 2 * 38);
        let gs = to_gaussians(&out, ShDegree::Two);
        assert_eq!(gs[0].sh.len(), 27);
    }

    #[test]
    fn surface_targets_are_on_surface() {
        let g = VoxelGrid::cube(8, 2);
        let t = surface_targets(&g, 16);
        assert_eq!(t.len(), 16);
        for s in &t {
            assert!(s.pos.iter().all(|v| (-1.0..=1.0).contains(v)));
            assert_eq!(s.opacity, 1.0);
            assert_eq!(s.rotation, [1.0, 0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn ply_header_and_rows() {
        let gs = surface_targets(&VoxelGrid::cube(8, 2), 4);
        let p = std::env::temp_dir().join("trit-3d-ply-test.ply");
        write_ply(&gs, &p).unwrap();
        let s = std::fs::read_to_string(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert!(s.starts_with(
            "ply
format ascii 1.0
"
        ));
        assert!(s.contains("element vertex 4"));
        let body = s
            .split(
                "end_header
",
            )
            .nth(1)
            .expect("缺 end_header");
        assert_eq!(body.lines().filter(|l| !l.is_empty()).count(), 4);
    }

    #[test]
    fn gaussian_training_reduces_loss() {
        let g = VoxelGrid::cube(8, 2);
        let mut enc = DenseEncoder::new(8, 64, 32, 11);
        let mut head = GaussianHead::with_degree(enc.latent_dim(), 32, ShDegree::One, 11);
        let r = train_gaussian_head(&mut enc, &mut head, &g, 200, 0.05);
        assert!(
            r.last_loss < r.first_loss,
            "MSE 未下降: {} -> {}",
            r.first_loss,
            r.last_loss
        );
        assert!(
            r.last_color < r.first_color,
            "颜色 MSE 未下降: {} -> {}",
            r.first_color,
            r.last_color
        );
        assert!(
            r.last_chamfer <= r.first_chamfer,
            "Chamfer 未下降: {} -> {}",
            r.first_chamfer,
            r.last_chamfer
        );
        assert_eq!(r.params, 23);
    }

    #[test]
    fn training_makes_high_order_sh_nonzero() {
        let g = VoxelGrid::cube(8, 2);
        let mut enc = DenseEncoder::new(8, 64, 32, 11);
        let mut head = GaussianHead::with_degree(enc.latent_dim(), 16, ShDegree::Three, 11);
        let _ = train_gaussian_head(&mut enc, &mut head, &g, 300, 0.05);
        let out = head.forward(&enc.encode(&g));
        let gs = to_gaussians(&out, ShDegree::Three);
        let high: f32 = gs
            .iter()
            .map(|g| g.sh[3..].iter().map(|v| v.abs()).sum::<f32>())
            .sum();
        assert!(high > 1e-2, "高阶 SH 仍然接近全零: {high}");
    }
}
