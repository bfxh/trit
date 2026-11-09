//! 三态场扩散：在 [`SignField3D`]（Inside / Null / Outside）上做 DDPM。
//!
//! 与 `trit-3d` 三值资产组件的边界：`trit_3d::{DenseEncoder, VoxelHead}` 是**单步回归**
//! （`i8` VoxelGrid → latent → 体素/3DGS 参数），没有时间步条件；本模块的去噪网络吃的是
//! **连续**的 `x_t`（f32）+ 时间步嵌入，且每步都要反传。两者接口与数据语义不同，因此
//! 没有直接复用那些头，只复用了更底层的 `trit-learn` 三值层（TernaryDense/STE/Adam）。
//!
//! 这是「三进制 + 扩散」的正题：生成目标不是连续标量场，而是本仓固有的三态符号场
//! （2 bit/体素）。管线是
//!
//! 1. 三态场 → 连续编码（`-1 / 0 / +1`）→ 标准 DDPM 训练（去噪网络权重三值）；
//! 2. 采样得到连续向量 → 按带宽阈值化回三态场；
//! 3. 用 `inside` 集合的 IoU 与三态比例评估。
//!
//! 编码是可逆的（编码值恰好是 `TriSign::as_i8`），因此「三态 → 连续 → 三态」的
//! 往返在 `band < 0.5` 时无损；采样出来的连续值落在带内即判为 `Null`（表面）。
//!
//! # 实测边界（负结果，先读）
//!
//! `n = 6`（216 维）、5 个参考形状、hidden 128、6 万步、DDIM stride 1：
//! 三值 loss 0.511 → 0.321、生成场 `inside` IoU 仅 **0.128**（FP 0.073，三值更好但同样
//! 没学会）。归因：① 参考 `inside` 只占 4.4%（约 9.5/216 体素），信号极稀疏；
//! ② 5 个形状各 1 样本、无增广；③ 纯 MLP 对 3D 场没有空间结构先验。
//! 下一步应优先换结构化网络（卷积/注意力/八叉树）或加大 `n` 并做形状增广。

use trit_core::signfield::{SignField3D, TriSign};
use trit_learn::activation::Activation;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::mse;
use trit_learn::opt::{Adam, Optimizer};
use trit_learn::quant::Ste;

use crate::data::Xorshift;
use crate::denoiser::{EMB, time_emb};
use crate::schedule::DiffusionSchedule;

/// `n³` 场的编码长度。
pub fn field_dim(n: u32) -> usize {
    (n as usize).pow(3)
}

/// 三态场 → 连续编码（`Inside = -1, Null = 0, Outside = +1`）。
pub fn encode(field: &SignField3D) -> Vec<f32> {
    let n = field.n();
    let mut out = Vec::with_capacity(field_dim(n));
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                out.push(field.at(x, y, z).as_i8() as f32);
            }
        }
    }
    out
}

/// 连续向量 → 三态场：`v < -band ⇒ Inside`，`v > band ⇒ Outside`，其余 `Null`。
///
/// `band` 取 0.5 时对编码值无损（编码只有 -1/0/+1）。
pub fn decode(v: &[f32], n: u32, band: f32) -> SignField3D {
    assert_eq!(v.len(), field_dim(n), "编码长度必须等于 n³");
    let mut field = SignField3D::new(n);
    let mut i = 0;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let s = if v[i] < -band {
                    TriSign::Inside
                } else if v[i] > band {
                    TriSign::Outside
                } else {
                    TriSign::Null
                };
                field.set(x, y, z, s);
                i += 1;
            }
        }
    }
    field
}

/// 用解析 SDF 生成三态场（体素中心采样，表面带 `band` 厚的 `Null` 层）。
fn from_sdf(n: u32, band: f32, sdf: impl Fn(f32, f32, f32) -> f32) -> SignField3D {
    let mut field = SignField3D::new(n);
    let c = n as f32 / 2.0;
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let d = sdf(x as f32 + 0.5 - c, y as f32 + 0.5 - c, z as f32 + 0.5 - c);
                let s = if d < -band {
                    TriSign::Inside
                } else if d > band {
                    TriSign::Outside
                } else {
                    TriSign::Null
                };
                field.set(x, y, z, s);
            }
        }
    }
    field
}

/// 球（半径以体素为单位，带厚 `band`）。
pub fn sphere(n: u32, radius: f32, band: f32) -> SignField3D {
    from_sdf(n, band, |x, y, z| {
        (x * x + y * y + z * z).sqrt() - radius
    })
}

/// 轴对齐立方体（半边长 `half`）。
pub fn cube(n: u32, half: f32, band: f32) -> SignField3D {
    from_sdf(n, band, |x, y, z| {
        let q = [
            x.abs() - half,
            y.abs() - half,
            z.abs() - half,
        ];
        let outside = [
            q[0].max(0.0),
            q[1].max(0.0),
            q[2].max(0.0),
        ];
        let len = (outside[0] * outside[0] + outside[1] * outside[1] + outside[2] * outside[2])
            .sqrt();
        len + q[0].max(q[1]).max(q[2]).min(0.0)
    })
}

/// 环面（主半径 `big`、管半径 `small`）。
pub fn torus(n: u32, big: f32, small: f32, band: f32) -> SignField3D {
    from_sdf(n, band, |x, y, z| {
        let q = (x * x + z * z).sqrt() - big;
        (q * q + y * y).sqrt() - small
    })
}

/// 三态比例 `[inside, null, outside]`（和为 1）。
pub fn ratios(field: &SignField3D) -> [f32; 3] {
    let c = field.counts();
    let total = (c[0] + c[1] + c[2]).max(1) as f32;
    [
        c[0] as f32 / total,
        c[1] as f32 / total,
        c[2] as f32 / total,
    ]
}

/// 两个场 `inside` 集合的 IoU（都为空时记 1）。
pub fn inside_iou(a: &SignField3D, b: &SignField3D) -> f32 {
    assert_eq!(a.n(), b.n(), "比较的两个场分辨率必须一致");
    let n = a.n();
    let (mut inter, mut union) = (0usize, 0usize);
    for z in 0..n {
        for y in 0..n {
            for x in 0..n {
                let ia = a.at(x, y, z) == TriSign::Inside;
                let ib = b.at(x, y, z) == TriSign::Inside;
                if ia && ib {
                    inter += 1;
                }
                if ia || ib {
                    union += 1;
                }
            }
        }
    }
    if union == 0 {
        1.0
    } else {
        inter as f32 / union as f32
    }
}

/// 通用维度的线性层（三值 QAT 或 FP），服务三态场的 `n³` 维输入/输出。
enum FieldLin {
    Ternary {
        layer: TernaryDense,
        opt_latent: Adam,
        opt_scale: Adam,
    },
    Fp {
        rows: usize,
        cols: usize,
        w: Vec<f32>,
        gw: Vec<f32>,
        opt: Adam,
        input: Vec<f32>,
        batch: usize,
    },
}

impl FieldLin {
    fn ternary(rows: usize, cols: usize, rng: &mut Xorshift, lr: f32) -> Self {
        let mut vals = Vec::with_capacity(rows * cols);
        for _ in 0..rows * cols {
            vals.push(0.06 * rng.normal());
        }
        let mut it = vals.into_iter();
        let layer = TernaryDense::new(
            rows,
            cols,
            move |_, _| it.next().unwrap_or(0.0),
            |_| 0.05,
            Ste::Identity,
        );
        Self::Ternary {
            layer,
            opt_latent: Adam::new(lr),
            opt_scale: Adam::new(lr),
        }
    }

    fn fp(rows: usize, cols: usize, rng: &mut Xorshift, lr: f32) -> Self {
        let w: Vec<f32> = (0..rows * cols).map(|_| 0.06 * rng.normal()).collect();
        Self::Fp {
            rows,
            cols,
            w,
            gw: vec![0.0; rows * cols],
            opt: Adam::new(lr),
            input: Vec::new(),
            batch: 0,
        }
    }

    fn forward(&mut self, batch: &[f32]) -> Vec<f32> {
        match self {
            Self::Ternary { layer, .. } => layer.forward(batch),
            Self::Fp {
                rows,
                cols,
                w,
                input,
                batch: t_slot,
                ..
            } => {
                assert!(
                    !batch.is_empty() && batch.len().is_multiple_of(*cols),
                    "输入长度必须是 cols={} 的非零整数倍",
                    cols
                );
                let t = batch.len() / *cols;
                input.clear();
                input.extend_from_slice(batch);
                *t_slot = t;
                let mut out = vec![0.0f32; t * *rows];
                for ti in 0..t {
                    let x = &batch[ti * *cols..(ti + 1) * *cols];
                    for r in 0..*rows {
                        let mut acc = 0.0f32;
                        for (c, &xc) in x.iter().enumerate() {
                            acc += w[r * *cols + c] * xc;
                        }
                        out[ti * *rows + r] = acc;
                    }
                }
                out
            }
        }
    }

    fn backward(&mut self, grad_out: &[f32]) -> Vec<f32> {
        match self {
            Self::Ternary { layer, .. } => layer.backward(grad_out),
            Self::Fp {
                rows,
                cols,
                w,
                gw,
                input,
                batch,
                ..
            } => {
                let t = *batch;
                assert_eq!(grad_out.len(), t * *rows, "grad_out 长度必须是 T×rows");
                let mut grad_in = vec![0.0f32; t * *cols];
                for ti in 0..t {
                    let x = &input[ti * *cols..(ti + 1) * *cols];
                    for r in 0..*rows {
                        let dy = grad_out[ti * *rows + r];
                        if dy == 0.0 {
                            continue;
                        }
                        for (c, &xc) in x.iter().enumerate() {
                            gw[r * *cols + c] += dy * xc;
                            grad_in[ti * *cols + c] += dy * w[r * *cols + c];
                        }
                    }
                }
                grad_in
            }
        }
    }

    fn zero_grad(&mut self) {
        match self {
            Self::Ternary { layer, .. } => layer.zero_grad(),
            Self::Fp { gw, .. } => gw.fill(0.0),
        }
    }

    fn step(&mut self) {
        match self {
            Self::Ternary {
                layer,
                opt_latent,
                opt_scale,
            } => layer.update(opt_latent, opt_scale),
            Self::Fp { w, gw, opt, .. } => opt.step(w, gw),
        }
    }

    fn rows(&self) -> usize {
        match self {
            Self::Ternary { layer, .. } => layer.rows(),
            Self::Fp { rows, .. } => *rows,
        }
    }

    fn cols(&self) -> usize {
        match self {
            Self::Ternary { layer, .. } => layer.cols(),
            Self::Fp { cols, .. } => *cols,
        }
    }
}

/// 三态场的去噪网络：`[v(n³), emb(t)] → Linear(h) → tanh → Linear(h) → tanh → Linear(n³)`。
pub struct FieldMlp {
    dim: usize,
    hidden: usize,
    ternary: bool,
    l1: FieldLin,
    l2: FieldLin,
    l3: FieldLin,
    b1: Vec<f32>,
    b2: Vec<f32>,
    b3: Vec<f32>,
    gb1: Vec<f32>,
    gb2: Vec<f32>,
    gb3: Vec<f32>,
    opt_b1: Adam,
    opt_b2: Adam,
    opt_b3: Adam,
    input: Vec<f32>,
    h1: Vec<f32>,
    h2: Vec<f32>,
    batch: usize,
}

impl FieldMlp {
    /// 三值权重版本。
    pub fn new(dim: usize, hidden: usize, seed: u64, lr: f32) -> Self {
        Self::build(dim, hidden, seed, lr, true)
    }

    /// 同初始化的 FP 对照。
    pub fn fp(dim: usize, hidden: usize, seed: u64, lr: f32) -> Self {
        Self::build(dim, hidden, seed, lr, false)
    }

    fn build(dim: usize, hidden: usize, seed: u64, lr: f32, ternary: bool) -> Self {
        assert!(dim > 0 && hidden > 0, "dim/hidden 必须非零");
        let in_dim = dim + EMB;
        let mut rng = Xorshift::new(seed);
        let (l1, l2, l3) = if ternary {
            (
                FieldLin::ternary(hidden, in_dim, &mut rng, lr),
                FieldLin::ternary(hidden, hidden, &mut rng, lr),
                FieldLin::ternary(dim, hidden, &mut rng, lr),
            )
        } else {
            (
                FieldLin::fp(hidden, in_dim, &mut rng, lr),
                FieldLin::fp(hidden, hidden, &mut rng, lr),
                FieldLin::fp(dim, hidden, &mut rng, lr),
            )
        };
        Self {
            dim,
            hidden,
            ternary,
            l1,
            l2,
            l3,
            b1: vec![0.0; hidden],
            b2: vec![0.0; hidden],
            b3: vec![0.0; dim],
            gb1: vec![0.0; hidden],
            gb2: vec![0.0; hidden],
            gb3: vec![0.0; dim],
            opt_b1: Adam::new(lr),
            opt_b2: Adam::new(lr),
            opt_b3: Adam::new(lr),
            input: Vec::new(),
            h1: Vec::new(),
            h2: Vec::new(),
            batch: 0,
        }
    }

    /// 编码维度 `n³`。
    pub fn dim(&self) -> usize {
        self.dim
    }

    fn forward_inner(&mut self, x: &[Vec<f32>], t: &[usize], steps: usize) -> Vec<Vec<f32>> {
        assert_eq!(x.len(), t.len(), "x 与 t 的批大小必须一致");
        assert!(!x.is_empty(), "批不能为空");
        let b = x.len();
        let (dim, h) = (self.dim, self.hidden);
        let in_dim = dim + EMB;
        let mut input = vec![0.0f32; b * in_dim];
        for i in 0..b {
            assert_eq!(x[i].len(), dim, "第 {i} 条样本维数应为 {dim}");
            input[i * in_dim..i * in_dim + dim].copy_from_slice(&x[i]);
            time_emb(t[i], steps, &mut input[i * in_dim + dim..(i + 1) * in_dim]);
        }
        let z1 = self.l1.forward(&input);
        let h1: Vec<f32> = z1
            .iter()
            .enumerate()
            .map(|(i, &v)| Activation::Tanh.forward(v + self.b1[i % h]))
            .collect();
        let z2 = self.l2.forward(&h1);
        let h2: Vec<f32> = z2
            .iter()
            .enumerate()
            .map(|(i, &v)| Activation::Tanh.forward(v + self.b2[i % h]))
            .collect();
        let z3 = self.l3.forward(&h2);
        let mut out = Vec::with_capacity(b);
        for i in 0..b {
            let mut row = Vec::with_capacity(dim);
            for d in 0..dim {
                row.push(z3[i * dim + d] + self.b3[d]);
            }
            out.push(row);
        }
        self.input = input;
        self.h1 = h1;
        self.h2 = h2;
        self.batch = b;
        out
    }

    fn backward_inner(&mut self, grad: &[Vec<f32>]) {
        let b = self.batch;
        let (dim, h) = (self.dim, self.hidden);
        assert_eq!(grad.len(), b, "grad 批大小必须与上次 forward 一致");
        let mut g3 = vec![0.0f32; b * dim];
        for i in 0..b {
            assert_eq!(grad[i].len(), dim, "第 {i} 条梯度维数应为 {dim}");
            for d in 0..dim {
                g3[i * dim + d] = grad[i][d];
                self.gb3[d] += grad[i][d];
            }
        }
        let g2pre = self.l3.backward(&g3);
        let mut dh2 = vec![0.0f32; b * h];
        for i in 0..b * h {
            let g = g2pre[i] * Activation::Tanh.backward(self.h2[i]);
            dh2[i] = g;
            self.gb2[i % h] += g;
        }
        let g1pre = self.l2.backward(&dh2);
        let mut dh1 = vec![0.0f32; b * h];
        for i in 0..b * h {
            let g = g1pre[i] * Activation::Tanh.backward(self.h1[i]);
            dh1[i] = g;
            self.gb1[i % h] += g;
        }
        let _ = self.l1.backward(&dh1);
    }

    fn update_inner(&mut self) {
        self.l1.step();
        self.l2.step();
        self.l3.step();
        self.opt_b1.step(&mut self.b1, &self.gb1);
        self.opt_b2.step(&mut self.b2, &self.gb2);
        self.opt_b3.step(&mut self.b3, &self.gb3);
    }

    fn zero_grad_inner(&mut self) {
        self.l1.zero_grad();
        self.l2.zero_grad();
        self.l3.zero_grad();
        self.gb1.fill(0.0);
        self.gb2.fill(0.0);
        self.gb3.fill(0.0);
    }

    /// 权重理论存储位数（三值 2 bit/权重 + 每行 f32 scale；FP 32 bit/权重）。
    pub fn weight_bits(&self) -> usize {
        let n = self.l1.rows() * self.l1.cols()
            + self.l2.rows() * self.l2.cols()
            + self.l3.rows() * self.l3.cols();
        if self.ternary {
            let scales = self.l1.rows() + self.l2.rows() + self.l3.rows();
            2 * n + 32 * scales
        } else {
            32 * n
        }
    }

    /// 是否三值。
    pub fn is_ternary(&self) -> bool {
        self.ternary
    }

    /// 前向（公开包装）。
    pub fn forward(&mut self, x: &[Vec<f32>], t: &[usize], steps: usize) -> Vec<Vec<f32>> {
        self.forward_inner(x, t, steps)
    }

    /// 反向（公开包装）。
    pub fn backward(&mut self, grad: &[Vec<f32>]) {
        self.backward_inner(grad);
    }

    /// 清零梯度。
    pub fn zero_grad(&mut self) {
        self.zero_grad_inner();
    }

    /// 应用一步更新。
    pub fn update(&mut self) {
        self.update_inner();
    }
}

/// 三态场训练超参数。
#[derive(Clone, Copy, Debug)]
pub struct FieldTrainOptions {
    /// 训练步数。
    pub steps: usize,
    /// 批大小。
    pub batch: usize,
    /// 随机种子。
    pub seed: u64,
    /// 每多少步抽样一次损失曲线（0 = 只记首尾）。
    pub log_every: usize,
}

impl Default for FieldTrainOptions {
    fn default() -> Self {
        Self {
            steps: 6000,
            batch: 8,
            seed: 7,
            log_every: 1000,
        }
    }
}

/// 三态场训练结果。
#[derive(Clone, Debug)]
pub struct FieldTrainReport {
    /// 第 1 步损失。
    pub initial_loss: f32,
    /// 最后一步损失。
    pub final_loss: f32,
    /// 抽样损失曲线（末尾必含最后一步）。
    pub loss_curve: Vec<f32>,
    /// 耗时（秒）。
    pub secs: f32,
}

/// 训练三态场去噪器（DDPM，预测噪声）。
pub fn train_field(
    model: &mut FieldMlp,
    sched: &DiffusionSchedule,
    data: &[Vec<f32>],
    opts: &FieldTrainOptions,
) -> FieldTrainReport {
    assert!(!data.is_empty(), "三态场训练数据不能为空");
    assert!(opts.steps > 0 && opts.batch > 0, "steps/batch 必须非零");
    let dim = model.dim();
    for (i, row) in data.iter().enumerate() {
        assert_eq!(row.len(), dim, "第 {i} 条训练场维数应为 {dim}");
    }
    let t_max = sched.steps();
    let mut rng = Xorshift::new(opts.seed);
    let start = std::time::Instant::now();
    let mut curve = Vec::new();
    let (mut first, mut last) = (f32::NAN, f32::NAN);

    for step in 0..opts.steps {
        let b = opts.batch;
        let mut x0 = Vec::with_capacity(b);
        for _ in 0..b {
            let idx = (rng.next_u64() as usize) % data.len();
            x0.push(data[idx].clone());
        }
        let mut t = Vec::with_capacity(b);
        for _ in 0..b {
            t.push((rng.next_u64() as usize) % t_max);
        }
        let mut eps = Vec::with_capacity(b);
        for _ in 0..b {
            eps.push((0..dim).map(|_| rng.normal()).collect::<Vec<f32>>());
        }
        let mut xt = Vec::with_capacity(b);
        for i in 0..b {
            let ab = sched.alpha_bar(t[i]);
            let s0 = ab.sqrt();
            let s1 = (1.0 - ab).max(0.0).sqrt();
            let row: Vec<f32> = (0..dim).map(|d| s0 * x0[i][d] + s1 * eps[i][d]).collect();
            xt.push(row);
        }
        let pred = model.forward(&xt, &t, t_max);
        let mut pf = Vec::with_capacity(b * dim);
        let mut ef = Vec::with_capacity(b * dim);
        for i in 0..b {
            pf.extend_from_slice(&pred[i]);
            ef.extend_from_slice(&eps[i]);
        }
        let (loss, grad_flat) = mse(&pf, &ef);
        assert!(loss.is_finite(), "第 {step} 步损失不是有限值");
        let mut grad = Vec::with_capacity(b);
        for i in 0..b {
            grad.push(grad_flat[i * dim..(i + 1) * dim].to_vec());
        }
        model.zero_grad();
        model.backward(&grad);
        model.update();

        if step == 0 {
            first = loss;
        }
        last = loss;
        if opts.log_every > 0 && step % opts.log_every == 0 {
            curve.push(loss);
        }
    }
    curve.push(last);
    FieldTrainReport {
        initial_loss: first,
        final_loss: last,
        loss_curve: curve,
        secs: start.elapsed().as_secs_f32(),
    }
}

/// DDIM 采样三态场（确定性，`stride` 为跳步间隔）。
pub fn sample_fields(
    model: &mut FieldMlp,
    sched: &DiffusionSchedule,
    n_samples: usize,
    stride: usize,
    rng: &mut Xorshift,
) -> Vec<Vec<f32>> {
    assert!(n_samples > 0, "采样数必须非零");
    let dim = model.dim();
    let steps = sched.steps();
    let stride = stride.max(1);
    let mut x: Vec<Vec<f32>> = (0..n_samples)
        .map(|_| (0..dim).map(|_| rng.normal()).collect())
        .collect();
    let mut ti = steps - 1;
    loop {
        let t = vec![ti; n_samples];
        let pred = model.forward(&x, &t, steps);
        let ab = sched.alpha_bar(ti).max(1e-12);
        let (prev, ab_prev) = if ti >= stride {
            (ti - stride, sched.alpha_bar(ti - stride))
        } else {
            (0, 1.0)
        };
        let inv_ab = 1.0 / ab.sqrt();
        let coef = (1.0 - ab).max(0.0).sqrt();
        for i in 0..n_samples {
            for d in 0..dim {
                let x0 = ((x[i][d] - coef * pred[i][d]) * inv_ab).clamp(-1.5, 1.5);
                x[i][d] = ab_prev.sqrt() * x0 + (1.0 - ab_prev).max(0.0).sqrt() * pred[i][d];
            }
        }
        if ti < stride {
            break;
        }
        ti = prev;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schedule::Schedule;

    #[test]
    fn encode_decode_roundtrip_is_lossless() {
        let f = sphere(6, 2.0, 0.7);
        let v = encode(&f);
        let back = decode(&v, 6, 0.5);
        assert_eq!(f, back, "编码→解码必须无损");
    }

    #[test]
    fn sphere_has_three_states() {
        let f = sphere(6, 2.0, 0.7);
        let c = f.counts();
        assert!(c[0] > 0 && c[1] > 0 && c[2] > 0, "球应同时有内/表/外：{c:?}");
        let r = ratios(&f);
        assert!((r[0] + r[1] + r[2] - 1.0).abs() < 1e-5);
    }

    #[test]
    fn iou_is_one_for_self() {
        let a = cube(6, 1.5, 0.7);
        assert!((inside_iou(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn sphere_and_cube_differ() {
        // n 太小（如 6³）时球与立方的 inside 集合可能完全重合（各只有 8 个中心体素），
        // 用 10³ 才分得开——这也说明评估分辨率必须与形状尺度匹配。
        let s = sphere(10, 2.5, 0.7);
        let c = cube(10, 2.0, 0.7);
        let iou = inside_iou(&s, &c);
        assert!(iou > 0.0 && iou < 1.0, "球与立方 IoU 应在 (0,1)：{iou}");
    }

    #[test]
    fn field_model_forward_shape_and_ternary() {
        let dim = field_dim(4);
        let mut m = FieldMlp::new(dim, 8, 3, 0.01);
        let x = vec![vec![0.0f32; dim]; 3];
        let t = [0usize, 5, 11];
        let out = m.forward(&x, &t, 12);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|r| r.len() == dim && r.iter().all(|v| v.is_finite())));
        assert!(m.is_ternary());
        assert!(m.weight_bits() > 0);
    }

    #[test]
    fn field_training_reduces_loss() {
        let dim = field_dim(4);
        let data: Vec<Vec<f32>> = [1.0f32, 1.5, 2.0]
            .iter()
            .map(|r| encode(&sphere(4, *r, 0.6)))
            .collect();
        let sched = DiffusionSchedule::new(Schedule::Cosine, 16);
        let mut m = FieldMlp::new(dim, 16, 5, 0.02);
        let rep = train_field(
            &mut m,
            &sched,
            &data,
            &FieldTrainOptions {
                steps: 60,
                batch: 4,
                seed: 2,
                log_every: 10,
            },
        );
        assert!(rep.final_loss < rep.initial_loss * 0.95, "三态场训练损失未下降");
    }

    #[test]
    fn field_sampling_is_finite() {
        let dim = field_dim(4);
        let mut m = FieldMlp::new(dim, 8, 9, 0.01);
        let sched = DiffusionSchedule::linear(8);
        let mut rng = Xorshift::new(4);
        let out = sample_fields(&mut m, &sched, 2, 2, &mut rng);
        assert_eq!(out.len(), 2);
        assert!(out.iter().flatten().all(|v| v.is_finite()));
    }
}
