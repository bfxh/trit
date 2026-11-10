//! 批量三值 QAT 全连接层：`Y[t,r] = scale[r] · Σ_c q[r,c]·X[t,c]`。
//!
//! 与 `trit-train::layer::TernaryLinear`（单样本 SGD 探针）不同，本层是给
//! **多层堆叠 + 批训练**用的共用原语：
//!
//! - 前向始终用三值权重 `q = clamp(round(w/scale), -1, 1)`（QAT，训练完没有
//!   「导出再量化」的精度落差，只有 per-row scale 是 f32）；
//! - 反向用 [`Ste`] 把 `∂L/∂q` 直通回 latent，scale 梯度用 [`scale_grad`]；
//! - 梯度在层内累积（多层堆叠时每层各调一次 [`TernaryDense::backward`]），
//!   由外部 [`Optimizer`] 统一应用。
//!
//! 本层不带偏置（M0 边界）：注意力投影与 FFN 在小任务上不需要它。

use crate::opt::Optimizer;
use crate::quant::{Ste, quantize_with_scale, scale_grad};

/// scale 允许的最小值（与 `trit-train` 的约定一致，防止除零与符号翻转）。
const MIN_SCALE: f32 = 1e-4;

/// 批量三值 QAT 全连接层。
#[derive(Clone, Debug)]
pub struct TernaryDense {
    rows: usize,
    cols: usize,
    /// latent 权重，行主序 `rows × cols`。
    latent: Vec<f32>,
    /// 每行 scale。
    scale: Vec<f32>,
    ste: Ste,
    /// 最近一次 forward 的输入批（`T × cols` 行主序），backward 需要。
    input: Vec<f32>,
    /// 最近一次 forward 的批行数 `T`。
    batch_rows: usize,
    /// 累积的 latent 梯度（`∂L/∂w`，已过 STE 门）。
    grad_latent: Vec<f32>,
    /// 累积的 scale 梯度（`∂L/∂scale`）。
    grad_scale: Vec<f32>,
}

impl TernaryDense {
    /// 新建：`init(r, c)` 给 latent 初值，每行初始 scale 由 `scale0(r)` 给出。
    pub fn new(
        rows: usize,
        cols: usize,
        mut init: impl FnMut(usize, usize) -> f32,
        scale0: impl Fn(usize) -> f32,
        ste: Ste,
    ) -> Self {
        assert!(rows > 0 && cols > 0, "rows/cols 必须非零");
        let mut latent = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                latent.push(init(r, c));
            }
        }
        Self {
            rows,
            cols,
            latent,
            scale: (0..rows).map(|r| scale0(r).max(MIN_SCALE)).collect(),
            ste,
            input: Vec::new(),
            batch_rows: 0,
            grad_latent: vec![0.0; rows * cols],
            grad_scale: vec![0.0; rows],
        }
    }

    /// 行数（输出维数）。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数（输入维数）。
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// latent 权重（行主序）。
    pub fn latent(&self) -> &[f32] {
        &self.latent
    }

    /// 每行 scale。
    pub fn scales(&self) -> &[f32] {
        &self.scale
    }

    /// 累积的 latent 梯度。
    pub fn grad_latent(&self) -> &[f32] {
        &self.grad_latent
    }

    /// 累积的 scale 梯度。
    pub fn grad_scale(&self) -> &[f32] {
        &self.grad_scale
    }

    /// 第 `r` 行的三值权重。
    pub fn quantized_row(&self, r: usize) -> Vec<i8> {
        quantize_with_scale(
            &self.latent[r * self.cols..(r + 1) * self.cols],
            self.scale[r],
        )
    }

    /// 全部三值权重（行主序）。
    pub fn quantized(&self) -> Vec<Vec<i8>> {
        (0..self.rows).map(|r| self.quantized_row(r)).collect()
    }

    /// 批前向：`batch` 是 `T × cols` 行主序扁平数组，返回 `T × rows`。
    ///
    /// 输入批会被缓存到下一次 forward。
    pub fn forward(&mut self, batch: &[f32]) -> Vec<f32> {
        assert!(
            !batch.is_empty() && batch.len().is_multiple_of(self.cols),
            "输入长度 {} 必须是 cols={} 的非零整数倍",
            batch.len(),
            self.cols
        );
        let rows_of_batch = batch.len() / self.cols;
        self.input.clear();
        self.input.extend_from_slice(batch);
        self.batch_rows = rows_of_batch;
        let mut out = vec![0.0f32; rows_of_batch * self.rows];
        for t in 0..rows_of_batch {
            let x = &batch[t * self.cols..(t + 1) * self.cols];
            for r in 0..self.rows {
                let q = self.quantized_row(r);
                let dot: f32 = q
                    .iter()
                    .zip(x.iter())
                    .map(|(&qi, &xi)| f32::from(qi) * xi)
                    .sum();
                out[t * self.rows + r] = self.scale[r] * dot;
            }
        }
        out
    }

    /// 批反向：`grad_out` 是上游对本次 forward 输出（`T × rows`）的梯度；
    /// 返回对输入的梯度（`T × cols`），并把 `∂L/∂w`、`∂L/∂scale` 累加进层内梯度。
    ///
    /// 必须紧跟在同形状的 [`TernaryDense::forward`] 之后调用。
    pub fn backward(&mut self, grad_out: &[f32]) -> Vec<f32> {
        assert_eq!(
            grad_out.len(),
            self.batch_rows * self.rows,
            "grad_out 长度必须是 T×rows={}，实际 {}",
            self.batch_rows * self.rows,
            grad_out.len()
        );
        let mut grad_in = vec![0.0f32; self.batch_rows * self.cols];
        for t in 0..self.batch_rows {
            let x = &self.input[t * self.cols..(t + 1) * self.cols];
            for r in 0..self.rows {
                let dy = grad_out[t * self.rows + r];
                if dy == 0.0 {
                    continue;
                }
                let q = self.quantized_row(r);
                let base = r * self.cols;
                // ∂L/∂scale = Σ_t dy · (q·x)（前向 y = scale·Σq·x，q 对 scale 视为常数）。
                self.grad_scale[r] += dy * scale_grad(&q, x);
                for (c, &xc) in x.iter().enumerate() {
                    // ∂L/∂q[r,c] = Σ_t dy_t · scale_r · x_t,c，再过 STE 门回 latent。
                    let grad_q = dy * self.scale[r] * xc;
                    let idx = base + c;
                    self.grad_latent[idx] +=
                        self.ste.backward(self.latent[idx], self.scale[r], grad_q);
                    // STE 把 dq/dw 当 1 ⇒ 对输入的梯度直接用 q。
                    grad_in[t * self.cols + c] += dy * self.scale[r] * f32::from(q[c]);
                }
            }
        }
        grad_in
    }

    /// 清零累积梯度（新一步开始时调用）。
    pub fn zero_grad(&mut self) {
        self.grad_latent.fill(0.0);
        self.grad_scale.fill(0.0);
    }

    /// 用外部优化器应用累积梯度：latent 与 scale 各自一个优化器实例。
    ///
    /// 应用后 scale 被夹回 `≥ MIN_SCALE`，**梯度不清零**——调用方负责
    /// [`TernaryDense::zero_grad`]（两层分工让「调试时查看梯度」成为可能）。
    pub fn update(&mut self, latent_opt: &mut dyn Optimizer, scale_opt: &mut dyn Optimizer) {
        latent_opt.step(&mut self.latent, &self.grad_latent);
        scale_opt.step(&mut self.scale, &self.grad_scale);
        for scale in self.scale.iter_mut() {
            *scale = scale.max(MIN_SCALE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opt::{Adam, Sgd};

    struct Rng(u64);

    impl Rng {
        fn f32(&mut self) -> f32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            ((self.0 % 2000) as f32) / 1000.0 - 1.0
        }
    }

    /// 以 MSE 为终点的端到端损失：forward + mse。
    /// `mse` 返回的梯度已经是 `dL/dy`（对全部元素的均值形式），直接作 grad_out。
    fn mse_loss(layer: &mut TernaryDense, batch: &[f32], target: &[f32]) -> (f32, Vec<f32>) {
        let y = layer.forward(batch);
        crate::loss::mse(&y, target)
    }

    /// STE 的「代理前向」：把 `q` 换成 latent 本身。Identity STE 的解析梯度
    /// 恰好是这个光滑函数的梯度——对 w 的数值检查必须对着它做
    /// （真前向对 w 是分片常数，数值导数恒为 0，检查不出 STE 的链式结构）。
    fn surrogate_loss(layer: &TernaryDense, batch: &[f32], target: &[f32]) -> f32 {
        let rows_of_batch = batch.len() / layer.cols;
        let mut y = vec![0.0f32; rows_of_batch * layer.rows];
        for t in 0..rows_of_batch {
            for r in 0..layer.rows {
                let dot: f32 = layer.latent[r * layer.cols..(r + 1) * layer.cols]
                    .iter()
                    .zip(batch[t * layer.cols..(t + 1) * layer.cols].iter())
                    .map(|(&w, &x)| w * x)
                    .sum();
                y[t * layer.rows + r] = layer.scale[r] * dot;
            }
        }
        crate::loss::mse(&y, target).0
    }

    #[test]
    fn forward_matches_manual_computation() {
        // w = [[1, 0], [-1, 1]]，scale = [2, 0.5]；x 两行。
        let mut layer = TernaryDense::new(
            2,
            2,
            |r, c| match (r, c) {
                (0, 0) => 2.0,  // round(2/2)=1
                (0, 1) => 0.4,  // round(0.4/2)=0
                (1, 0) => -1.0, // round(-1/0.5)=-1
                (1, 1) => 0.6,  // round(0.6/0.5)=1
                _ => unreachable!(),
            },
            |r| if r == 0 { 2.0 } else { 0.5 },
            Ste::Identity,
        );
        let batch = [1.0, -2.0, 0.5, 0.5];
        let y = layer.forward(&batch);
        // 输出按 [t*rows + r] 排布：
        // t=0（x=[1,-2]）：行0 = 2·(1·1+0·(-2)) = 2；行1 = 0.5·(-1-2) = -1.5
        // t=1（x=[0.5,0.5]）：行0 = 2·0.5 = 1；行1 = 0.5·(-0.5+0.5) = 0
        assert_eq!(layer.quantized(), vec![vec![1, 0], vec![-1, 1]]);
        assert!((y[0] - 2.0).abs() < 1e-6, "y[0]={}", y[0]);
        assert!((y[1] + 1.5).abs() < 1e-6, "y[1]={}", y[1]);
        assert!((y[2] - 1.0).abs() < 1e-6, "y[2]={}", y[2]);
        assert!(y[3].abs() < 1e-6, "y[3]={}", y[3]);
    }

    #[test]
    fn gradients_match_finite_differences() {
        // 梯度检查分三路：
        // - dW 对「代理前向」（q := w）做中心差分。STE 的解析梯度正是这个
        //   光滑函数的梯度——真前向对 w 是分片常数，数值导数恒为 0。
        //   为让代理在中心点与真前向**取值一致**，latent 直接取 ±1（scale=1）。
        // - dX / dscale 对真前向做中心差分：x、scale 的扰动不改变 q，数值导数有效。
        let (rows, cols, t) = (3usize, 4usize, 2usize);
        let mut rng = Rng(0x1234_5678_9ABC_DEF0);
        let mut layer = TernaryDense::new(
            rows,
            cols,
            |r, c| if (r * cols + c) % 2 == 0 { 1.0 } else { -1.0 },
            |_| 1.0,
            Ste::Identity,
        );
        let batch: Vec<f32> = (0..t * cols).map(|_| rng.f32()).collect();
        let target: Vec<f32> = (0..t * rows).map(|_| rng.f32()).collect();

        let (_, grad_out) = mse_loss(&mut layer, &batch, &target);
        let grad_in = layer.backward(&grad_out);

        let h = 1e-3;
        // dW：对代理前向抽 5 个权重做中心差分。
        for &idx in &[0usize, 3, 5, 9, 11] {
            let saved = layer.latent[idx];
            let mut up = layer.clone();
            up.latent[idx] = saved + h;
            let mut down = layer.clone();
            down.latent[idx] = saved - h;
            let numeric = (surrogate_loss(&up, &batch, &target)
                - surrogate_loss(&down, &batch, &target))
                / (2.0 * h);
            let analytic = layer.grad_latent[idx];
            assert!(
                (analytic - numeric).abs() < 1e-3,
                "dW[{idx}]：解析 {analytic} vs 数值 {numeric}"
            );
        }
        // dX：对真前向抽 4 个输入位。
        for &idx in &[0usize, 2, 5, 7] {
            let mut up = batch.clone();
            up[idx] += h;
            let mut down = batch.clone();
            down[idx] -= h;
            let numeric = (mse_loss(&mut layer.clone(), &up, &target).0
                - mse_loss(&mut layer.clone(), &down, &target).0)
                / (2.0 * h);
            assert!(
                (grad_in[idx] - numeric).abs() < 1e-3,
                "dX[{idx}]：解析 {} vs 数值 {numeric}",
                grad_in[idx]
            );
        }
        // dscale：对真前向抽第 0 行（q 不翻转 ⇒ 数值导数有效）。
        let saved = layer.scales()[0];
        let mut up = layer.clone();
        up.scale[0] = saved + h;
        let mut down = layer.clone();
        down.scale[0] = saved - h;
        let numeric = (mse_loss(&mut up, &batch, &target).0
            - mse_loss(&mut down, &batch, &target).0)
            / (2.0 * h);
        assert!(
            (layer.grad_scale()[0] - numeric).abs() < 1e-3,
            "dscale：解析 {} vs 数值 {numeric}",
            layer.grad_scale()[0]
        );
    }

    #[test]
    fn clipped_ste_zeroes_saturated_gradients() {
        // |w/s| 超过 ratio 的权重不该收到梯度。
        let mut layer = TernaryDense::new(
            1,
            2,
            |_, c| if c == 0 { 5.0 } else { 0.1 },
            |_| 1.0,
            Ste::Clipped { ratio: 1.0 },
        );
        layer.forward(&[1.0, 1.0]);
        layer.backward(&[1.0]);
        assert_eq!(layer.grad_latent()[0], 0.0, "饱和权重梯度应为 0");
        assert!(layer.grad_latent()[1] != 0.0, "未饱和权重应有梯度");
    }

    #[test]
    fn accumulates_across_backward_calls() {
        let mut layer = TernaryDense::new(1, 2, |_, _| 0.7, |_| 1.0, Ste::Identity);
        layer.forward(&[1.0, 1.0]);
        layer.backward(&[1.0]);
        let after_one = layer.grad_latent().to_vec();
        layer.zero_grad();
        assert!(layer.grad_latent().iter().all(|&g| g == 0.0));
        layer.forward(&[1.0, 1.0]);
        layer.backward(&[1.0]);
        layer.forward(&[1.0, 1.0]);
        layer.backward(&[1.0]);
        for (twice, once) in layer.grad_latent().iter().zip(after_one.iter()) {
            assert!((twice - 2.0 * once).abs() < 1e-6, "两次 backward 应翻倍");
        }
    }

    #[test]
    fn update_descends_and_clamps_scale() {
        // 目标：y = 2·x（行 scale=2，q=[1]）。从 latent=0.7（q=1，scale=1）起步。
        let mut layer = TernaryDense::new(1, 1, |_, _| 0.7, |_| 1.0, Ste::Identity);
        let mut latent_opt = Sgd::new(0.2, 0.0);
        let mut scale_opt = Adam::new(0.05);
        let mut first_loss = f32::NAN;
        for step in 0..300 {
            layer.zero_grad();
            let (loss, grad) = mse_loss(&mut layer, &[1.0], &[2.0]);
            if step == 0 {
                first_loss = loss;
            }
            layer.backward(&grad);
            layer.update(&mut latent_opt, &mut scale_opt);
        }
        layer.zero_grad();
        let (final_loss, _) = mse_loss(&mut layer, &[1.0], &[2.0]);
        assert!(final_loss < first_loss * 0.5, "{first_loss} → {final_loss}");
        assert!(
            (layer.scales()[0] - 2.0).abs() < 0.3,
            "scale={:?}",
            layer.scales()
        );
        // scale 不会被夹出正下界。
        assert!(layer.scales()[0] >= MIN_SCALE);
    }

    #[test]
    #[should_panic(expected = "整数倍")]
    fn forward_rejects_bad_batch_length() {
        let mut layer = TernaryDense::new(1, 3, |_, _| 0.5, |_| 1.0, Ste::Identity);
        let _ = layer.forward(&[1.0, 2.0]);
    }

    #[test]
    #[should_panic(expected = "T×rows")]
    fn backward_rejects_wrong_grad_shape() {
        let mut layer = TernaryDense::new(2, 2, |_, _| 0.5, |_| 1.0, Ste::Identity);
        let _ = layer.forward(&[1.0, 1.0, 1.0, 1.0]);
        let _ = layer.backward(&[1.0]);
    }
}
