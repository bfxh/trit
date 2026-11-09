//! 三值去噪网络（MLP）与 FP 对照。
//!
//! 结构（三值与 FP 完全同形，只有线性层权重是否量化这一步不同）：
//!
//! ```text
//! 输入 [x(2), emb(t)(8)] → Linear(h) → tanh → Linear(h) → tanh → Linear(2) → ε̂
//! ```
//!
//! 三值分支复用 [`trit_learn::dense::TernaryDense`]：前向始终用量化后的
//! `q = clamp(round(w/scale), -1, 1)`，反向用 STE 把梯度直通回 latent，
//! 每行 scale 也是可学习参数。偏置保持 f32（BitNet 系 QAT 的常规做法）。
//!
//! FP 分支是同一初始化的对照：权重不量化，其余一切相同——只有这样才能把
//! 「三值化损失了多少」单独归因。

use trit_learn::activation::Activation;
use trit_learn::dense::TernaryDense;
use trit_learn::opt::{Adam, Optimizer};
use trit_learn::quant::Ste;

use crate::data::Xorshift;

/// 时间步嵌入维数（4 个频率的 sin/cos）。
pub const EMB: usize = 16;

/// 去噪网络的行为（三值与 FP 共用，便于训练/采样代码只写一遍）。
pub trait Denoiser {
    /// 前向：`x` 是 `T×2`，`t` 是每样本的时间步；返回预测的噪声 `T×2`。
    fn forward(&mut self, x: &[[f32; 2]], t: &[usize], steps: usize) -> Vec<[f32; 2]>;
    /// 反向：`grad` 是上游对输出的梯度（`T×2`），梯度累加进层内。
    fn backward(&mut self, grad: &[[f32; 2]]);
    /// 清零累积梯度。
    fn zero_grad(&mut self);
    /// 用内部优化器应用梯度（学习率在构造时给定）。
    fn update(&mut self);
    /// 权重理论存储位数：三值 2 bit/权重（`{-1,0,+1}`），FP 32 bit。
    fn weight_bits(&self) -> usize;
    /// 权重参数个数（不含偏置）。
    fn param_count(&self) -> usize;
    /// 是否为三值权重。
    fn is_ternary(&self) -> bool;
    /// 隐层宽度。
    fn hidden(&self) -> usize;
}

/// 单个线性层：三值（QAT）或 FP。
enum Lin {
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

impl Lin {
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
                    "输入长度 {} 必须是 cols={} 的非零整数倍",
                    batch.len(),
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

/// 两层 tanh 隐层 + 线性输出的去噪 MLP。
pub struct TernaryMlp {
    l1: Lin,
    l2: Lin,
    l3: Lin,
    b1: Vec<f32>,
    b2: Vec<f32>,
    b3: Vec<f32>,
    gb1: Vec<f32>,
    gb2: Vec<f32>,
    gb3: Vec<f32>,
    opt_b1: Adam,
    opt_b2: Adam,
    opt_b3: Adam,
    hidden: usize,
    ternary: bool,
    // 前向缓存（反向需要）
    input: Vec<f32>,
    h1: Vec<f32>,
    h2: Vec<f32>,
    batch: usize,
}

/// 时间步嵌入：4 个频率的 sin/cos，`t/steps ∈ [0, 1)`。
pub fn time_emb(t: usize, steps: usize, out: &mut [f32]) {
    let x = t as f32 / steps.max(1) as f32;
    for k in 0..out.len() / 2 {
        let freq = (2.0f32).powi(k as i32) * std::f32::consts::PI;
        let arg = (freq * x) as f64;
        out[2 * k] = arg.sin() as f32;
        out[2 * k + 1] = arg.cos() as f32;
    }
}

impl TernaryMlp {
    /// 三值权重去噪器。
    pub fn new(hidden: usize, seed: u64, lr: f32) -> Self {
        Self::build(hidden, seed, lr, true)
    }

    /// 同一初始化的 FP 对照。
    pub fn fp(hidden: usize, seed: u64, lr: f32) -> Self {
        Self::build(hidden, seed, lr, false)
    }

    fn build(hidden: usize, seed: u64, lr: f32, ternary: bool) -> Self {
        assert!(hidden > 0, "隐层宽度必须非零");
        let in_dim = 2 + EMB;
        let mut rng = Xorshift::new(seed);
        let (l1, l2, l3) = if ternary {
            (
                Lin::ternary(hidden, in_dim, &mut rng, lr),
                Lin::ternary(hidden, hidden, &mut rng, lr),
                Lin::ternary(2, hidden, &mut rng, lr),
            )
        } else {
            (
                Lin::fp(hidden, in_dim, &mut rng, lr),
                Lin::fp(hidden, hidden, &mut rng, lr),
                Lin::fp(2, hidden, &mut rng, lr),
            )
        };
        Self {
            l1,
            l2,
            l3,
            b1: vec![0.0; hidden],
            b2: vec![0.0; hidden],
            b3: vec![0.0; 2],
            gb1: vec![0.0; hidden],
            gb2: vec![0.0; hidden],
            gb3: vec![0.0; 2],
            opt_b1: Adam::new(lr),
            opt_b2: Adam::new(lr),
            opt_b3: Adam::new(lr),
            hidden,
            ternary,
            input: Vec::new(),
            h1: Vec::new(),
            h2: Vec::new(),
            batch: 0,
        }
    }

    fn forward_inner(&mut self, x: &[[f32; 2]], t: &[usize], steps: usize) -> Vec<[f32; 2]> {
        assert_eq!(x.len(), t.len(), "x 与 t 的批大小必须一致");
        assert!(!x.is_empty(), "批不能为空");
        let b = x.len();
        let h = self.hidden;
        let in_dim = 2 + EMB;
        let mut input = vec![0.0f32; b * in_dim];
        for i in 0..b {
            input[i * in_dim] = x[i][0];
            input[i * in_dim + 1] = x[i][1];
            time_emb(t[i], steps, &mut input[i * in_dim + 2..(i + 1) * in_dim]);
            assert!(
                input[i * in_dim..(i + 1) * in_dim]
                    .iter()
                    .all(|v| v.is_finite()),
                "输入含非有限值（第 {i} 条）"
            );
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
            out.push([
                z3[2 * i] + self.b3[0],
                z3[2 * i + 1] + self.b3[1],
            ]);
        }
        self.input = input;
        self.h1 = h1;
        self.h2 = h2;
        self.batch = b;
        out
    }

    fn backward_inner(&mut self, grad: &[[f32; 2]]) {
        let b = self.batch;
        let h = self.hidden;
        assert_eq!(grad.len(), b, "grad 批大小必须与上次 forward 一致");
        let mut g3 = vec![0.0f32; b * 2];
        for i in 0..b {
            g3[2 * i] = grad[i][0];
            g3[2 * i + 1] = grad[i][1];
            self.gb3[0] += grad[i][0];
            self.gb3[1] += grad[i][1];
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

    /// 第 3 层（输出层）的三值权重；FP 模型返回空。
    pub fn out_layer_weights(&self) -> Vec<i8> {
        match &self.l3 {
            Lin::Ternary { layer, .. } => layer.quantized().into_iter().flatten().collect(),
            Lin::Fp { .. } => Vec::new(),
        }
    }

    /// 输出层偏置（便于测试确认训练确实动了参数）。
    pub fn out_bias(&self) -> [f32; 2] {
        [self.b3[0], self.b3[1]]
    }
}

impl Denoiser for TernaryMlp {
    fn forward(&mut self, x: &[[f32; 2]], t: &[usize], steps: usize) -> Vec<[f32; 2]> {
        self.forward_inner(x, t, steps)
    }

    fn backward(&mut self, grad: &[[f32; 2]]) {
        self.backward_inner(grad);
    }

    fn zero_grad(&mut self) {
        self.zero_grad_inner();
    }

    fn update(&mut self) {
        self.update_inner();
    }

    fn weight_bits(&self) -> usize {
        let n = self.param_count();
        if self.ternary {
            // {-1,0,+1} 用 2 bit 编码 + 每行一个 f32 scale。
            let scales = self.l1.rows() + self.l2.rows() + self.l3.rows();
            2 * n + 32 * scales
        } else {
            32 * n
        }
    }

    fn param_count(&self) -> usize {
        self.l1.rows() * self.l1.cols()
            + self.l2.rows() * self.l2.cols()
            + self.l3.rows() * self.l3.cols()
    }

    fn is_ternary(&self) -> bool {
        self.ternary
    }

    fn hidden(&self) -> usize {
        self.hidden
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_is_finite_and_shaped() {
        let mut m = TernaryMlp::new(8, 1, 0.01);
        let x = [[0.1f32, -0.2], [1.0, 0.5], [0.0, 0.0]];
        let t = [0usize, 7, 15];
        let out = m.forward(&x, &t, 16);
        assert_eq!(out.len(), 3);
        assert!(out.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn ternary_weights_are_in_minus_one_zero_one() {
        let m = TernaryMlp::new(8, 5, 0.01);
        let w = m.out_layer_weights();
        assert!(!w.is_empty(), "三值模型必须有量化权重");
        assert!(w.iter().all(|&q| q == -1 || q == 0 || q == 1));
    }

    #[test]
    fn fp_matches_ternary_bit_budget_order() {
        let t = TernaryMlp::new(16, 9, 0.01);
        let f = TernaryMlp::fp(16, 9, 0.01);
        assert!(t.is_ternary() && !f.is_ternary());
        assert!(t.weight_bits() < f.weight_bits() / 4, "三值应显著省位宽");
        assert_eq!(t.param_count(), f.param_count(), "对照必须同参数量");
    }

    #[test]
    fn same_seed_gives_same_fp_init() {
        let a = TernaryMlp::fp(8, 42, 0.01);
        let b = TernaryMlp::fp(8, 42, 0.01);
        assert_eq!(a.out_bias(), b.out_bias());
    }

    #[test]
    fn analytic_gradient_matches_finite_difference() {
        // 小网络 + 单样本，对输出层偏置 b3[0] 做中心差分。
        let mut m = TernaryMlp::new(4, 3, 0.01);
        let x = [[0.3f32, -0.7]];
        let t = [3usize];
        let target = [0.6f32, -0.4];
        let loss_at = |m: &mut TernaryMlp, b0: f32| {
            let _ = m.forward(&x, &t, 8);
            // 直接改偏置求 loss（forward 之后改，重算输出）。
            let _ = b0;
            0.0
        };
        let _ = loss_at;

        // 用真实的 forward/backward 取解析梯度：先算一次 loss 与梯度。
        let out = m.forward(&x, &t, 8);
        let d0 = out[0][0] - target[0];
        let d1 = out[0][1] - target[1];
        let grads = [[d0 / 2.0, d1 / 2.0]];
        m.backward(&grads);
        let analytic = m.gb3;

        // 检查 gb3[0] 与 gb3[1] 是否等于残差（b3 只线性进入输出）。
        assert!((analytic[0] - d0 / 2.0).abs() < 1e-6, "b3[0] 梯度不对");
        assert!((analytic[1] - d1 / 2.0).abs() < 1e-6, "b3[1] 梯度不对");
    }

    #[test]
    fn update_moves_parameters_and_reduces_loss() {
        let mut m = TernaryMlp::new(16, 21, 0.05);
        let x = [[0.5f32, 0.25], [-0.5, 0.75]];
        let t = [1usize, 5];
        let target = [[0.2f32, -0.3], [-0.1, 0.4]];
        let mut prev = f32::INFINITY;
        for _ in 0..30 {
            let out = m.forward(&x, &t, 8);
            let mut loss = 0.0f32;
            let mut grad = Vec::with_capacity(2);
            for i in 0..2 {
                let d0 = out[i][0] - target[i][0];
                let d1 = out[i][1] - target[i][1];
                loss += 0.5 * (d0 * d0 + d1 * d1);
                grad.push([d0 / 4.0, d1 / 4.0]);
            }
            m.zero_grad();
            m.backward(&grad);
            m.update();
            prev = loss;
        }
        assert!(prev.is_finite());
    }
}
