//! 压缩保真度诊断：f32 基准、投影路径与三值化损失拆解。
//!
//! 这是 `examples/agree_real` 那条实验线的库化。诊断固定一组
//! `query`（f32 权重）+ `readout`（f32 读出）+ `basis`（正交基），
//! 对一批真实输入输出八列一致率：
//!
//! `[f32/f32, 权重三值, 输入三值, 两半都三值, 两半保留 scale, readout 三值,
//! 完整前向, 真实三值网络]`
//!
//! 所有三值化都按 absmean（`scale = mean|w|`、`round(w/scale)` 截断到 `-1..=1`），
//! 与 `Layer::to_layer` 同口径；诊断本身用 f64，只在精确半整数边界上与库的 f32
//! 口径可能差一例（`examples/agree_real_selftest` 有负结果）。

use crate::net::Net;
use crate::safetensors::Weights;
use crate::trit::Trit;
use crate::{to_wsp, to_wsp2};

/// 取符号：`>0 / <0 / =0` → `Pos / Neg / Zero`。
#[must_use]
pub fn sign(value: f64) -> Trit {
    if value > 0.0 {
        Trit::Pos
    } else if value < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

/// 未压缩 f32 判据 `sign(readout · (query·x))`：权重与输入都不三值化。
#[must_use]
pub fn reference(query: &Weights, readout: &[f32], x: &[f32]) -> Trit {
    let dim = query.cols;
    let mut score = 0.0f64;
    for row in 0..query.rows {
        let v: f64 = query.values[row * dim..(row + 1) * dim]
            .iter()
            .zip(x)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        score += f64::from(readout.get(row).copied().unwrap_or(0.0)) * v;
    }
    sign(score)
}

/// 投影到 `k` 维、但**不三值化**的同构网络（权重与输入都留 f32）。
pub struct Projected {
    dim: usize,
    k: usize,
    rows: usize,
    wk: Vec<f64>,
    proj: Vec<f64>,
    readout: Vec<f32>,
}

impl Projected {
    /// `basis` 是 `k × dim` 的基（每行一个方向，调用方保证正交归一）。
    #[must_use]
    pub fn new(query: &Weights, readout: &[f32], basis: &[Vec<f64>]) -> Self {
        let dim = query.cols;
        let rows = query.rows;
        let k = basis.len();
        let proj: Vec<f64> = basis.iter().flatten().copied().collect();
        let mut wk = vec![0.0f64; rows * k];
        for row in 0..rows {
            let w = &query.values[row * dim..(row + 1) * dim];
            for i in 0..k {
                let u = &proj[i * dim..(i + 1) * dim];
                wk[row * k + i] = u.iter().zip(w).map(|(a, b)| a * f64::from(*b)).sum::<f64>();
            }
        }
        Self {
            dim,
            k,
            rows,
            wk,
            proj,
            readout: readout[..rows.min(readout.len())].to_vec(),
        }
    }

    /// `(原维度, 投影维度)`。
    #[must_use]
    pub fn dims(&self) -> (usize, usize) {
        (self.dim, self.k)
    }

    /// 逐行的投影权重（`rows × k`，f32 值）。
    #[must_use]
    pub fn weights(&self) -> &[f64] {
        &self.wk
    }

    /// 把 f32 输入投影到 `k` 维。
    #[must_use]
    pub fn project_f32(&self, x: &[f32]) -> Vec<f64> {
        (0..self.k)
            .map(|i| {
                let u = &self.proj[i * self.dim..(i + 1) * self.dim];
                u.iter().zip(x).map(|(a, b)| a * f64::from(*b)).sum()
            })
            .collect()
    }

    /// 把三值输入投影到 `k` 维（`±1/0` 直接点积）。
    #[must_use]
    pub fn project_trit(&self, x: &[Trit]) -> Vec<f64> {
        (0..self.k)
            .map(|i| {
                let u = &self.proj[i * self.dim..(i + 1) * self.dim];
                u.iter().zip(x).map(|(a, t)| a * f64::from(t.to_i8())).sum()
            })
            .collect()
    }

    /// 用给定（可能被三值化的）权重与投影输入判定，readout 保持 f32。
    #[must_use]
    pub fn judge(&self, weights: &[f64], xk: &[f64]) -> Trit {
        let mut score = 0.0f64;
        for row in 0..self.rows {
            let w = &weights[row * self.k..(row + 1) * self.k];
            let v: f64 = w.iter().zip(xk).map(|(a, b)| a * b).sum();
            score += f64::from(self.readout[row]) * v;
        }
        sign(score)
    }
}

/// 一行 f64 的 absmean 三值化（丢 scale）；全零行返回全零。
#[must_use]
pub fn absmean_row(row: &[f64]) -> Vec<f64> {
    let scale = row.iter().map(|v| v.abs()).sum::<f64>() / row.len().max(1) as f64;
    row.iter()
        .map(|v| (v / scale.max(f64::MIN_POSITIVE)).round().clamp(-1.0, 1.0))
        .collect()
}

/// 投影权重逐行 absmean 三值化：`(丢 scale, 保留 scale)`。
#[must_use]
pub fn trit_weights(projected: &Projected) -> (Vec<f64>, Vec<f64>) {
    let (_, k) = projected.dims();
    let mut signed = Vec::new();
    let mut scaled = Vec::new();
    for row in projected.weights().chunks(k) {
        let scale = row.iter().map(|v| v.abs()).sum::<f64>() / row.len().max(1) as f64;
        for v in row {
            let q = (v / scale.max(f64::MIN_POSITIVE)).round().clamp(-1.0, 1.0);
            signed.push(q);
            scaled.push(q * scale);
        }
    }
    (signed, scaled)
}

/// 一次诊断：固定 `query` / `readout` / 基，对一批输入逐列统计。
pub struct Diagnosis<'a> {
    query: &'a Weights,
    projected: Projected,
    weights: Vec<f64>,
    trit: (Vec<f64>, Vec<f64>),
    readout: Vec<f32>,
    readout_trit: Vec<f64>,
    basis: &'a [Vec<f64>],
}

impl<'a> Diagnosis<'a> {
    /// 构造诊断；`basis` 每行一个方向，调用方保证正交归一。
    #[must_use]
    pub fn new(query: &'a Weights, readout: &'a [f32], basis: &'a [Vec<f64>]) -> Self {
        let projected = Projected::new(query, readout, basis);
        let weights = projected.weights().to_vec();
        let trit = trit_weights(&projected);
        let rows = query.rows.min(readout.len());
        let readout = readout[..rows].to_vec();
        let readout_trit = absmean_row(&readout.iter().map(|v| f64::from(*v)).collect::<Vec<_>>());
        Self {
            query,
            projected,
            weights,
            trit,
            readout,
            readout_trit,
            basis,
        }
    }

    /// 未压缩 f32 基准。
    #[must_use]
    pub fn reference(&self, x: &[f32]) -> Trit {
        reference(self.query, &self.readout, x)
    }

    /// 八列一致率；`tern_out` 是真实三值网络的输出（每样本一个 `Vec<Trit>`）。
    #[must_use]
    pub fn compare(
        &self,
        base: &[Trit],
        raw: &[&[f32]],
        inputs: &[Vec<Trit>],
        tern_out: &[Vec<Trit>],
    ) -> [f64; 8] {
        let (w_trit, w_scaled) = &self.trit;
        let n = base.len().min(raw.len()).min(inputs.len());
        let mut hits = [0usize; 8];
        for i in 0..n {
            let xk = self.projected.project_f32(raw[i]);
            let xt = self.projected.project_trit(&inputs[i]);
            hits[0] += usize::from(self.projected.judge(&self.weights, &xk) == base[i]);
            hits[1] += usize::from(self.projected.judge(w_trit, &xk) == base[i]);
            hits[2] += usize::from(self.projected.judge(&self.weights, &xt) == base[i]);
            hits[3] += usize::from(self.projected.judge(w_trit, &xt) == base[i]);
            hits[4] += usize::from(self.projected.judge(w_scaled, &xt) == base[i]);
            hits[5] += usize::from(self.judge_readout(&xt) == base[i]);
            hits[6] += usize::from(self.full_forward(&inputs[i]) == base[i]);
            hits[7] += usize::from(tern_out[i].as_slice() == [base[i]]);
        }
        hits.map(|h| h as f64 / n.max(1) as f64)
    }

    /// 完整三值前向：输入投影+取符号、第一层逐行取符号、readout 三值化后点积。
    ///
    /// 与 `Net::forward` 同口径（bias 为零、scale 不影响符号判决）。
    #[must_use]
    pub fn full_forward(&self, input: &[Trit]) -> Trit {
        let xt: Vec<f64> = self
            .basis
            .iter()
            .map(|u| {
                let acc: f64 = u
                    .iter()
                    .zip(input)
                    .map(|(a, t)| a * f64::from(t.to_i8()))
                    .sum();
                f64::from(sign(acc).to_i8())
            })
            .collect();
        let (_, k) = self.projected.dims();
        let mut score = 0.0f64;
        for (row, q) in self.readout_trit.iter().enumerate() {
            let w = &self.trit.0[row * k..(row + 1) * k];
            let v = sign(w.iter().zip(&xt).map(|(a, b)| a * b).sum());
            score += q * f64::from(v.to_i8());
        }
        sign(score)
    }

    /// 三值投影网络的输出，以及它与三值学生的一致率。
    ///
    /// # Panics
    ///
    /// `net.forward` 失败时 panic（形状由本诊断构造，正常不会失败）。
    #[must_use]
    pub fn agree(&self, student: &Net, net: &Net, inputs: &[Vec<Trit>]) -> (f64, Vec<Vec<Trit>>) {
        let mut same = 0usize;
        let mut out = Vec::with_capacity(inputs.len());
        for input in inputs {
            let reduced: Vec<Trit> = self
                .basis
                .iter()
                .map(|u| {
                    let acc: f64 = u
                        .iter()
                        .zip(input)
                        .map(|(a, t)| a * f64::from(t.to_i8()))
                        .sum();
                    sign(acc)
                })
                .collect();
            let got = net.forward(&reduced).expect("三值网络前向");
            if student.forward(input).expect("三值学生前向") == got {
                same += 1;
            }
            out.push(got);
        }
        (same as f64 / inputs.len().max(1) as f64, out)
    }

    fn judge_readout(&self, xk: &[f64]) -> Trit {
        let (_, k) = self.projected.dims();
        let mut score = 0.0f64;
        for (row, q) in self.readout_trit.iter().enumerate() {
            let w = &self.trit.0[row * k..(row + 1) * k];
            score += q * w.iter().zip(xk).map(|(a, b)| a * b).sum::<f64>();
        }
        sign(score)
    }
}

/// WSP 编码体积。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    /// 权重 + bias 的 trit 总数。
    pub trits: usize,
    /// WSP1 文本字节数。
    pub wsp1: usize,
    /// WSP2（2-bit 定长）字节数。
    pub wsp2: usize,
}

/// 量一个网络的编码体积。
pub fn measure(net: &Net) -> crate::Result<Size> {
    let trits: usize = net
        .layers()
        .iter()
        .map(|layer| layer.trits().len() + layer.bias().len())
        .sum();
    Ok(Size {
        trits,
        wsp1: to_wsp(net).len(),
        wsp2: to_wsp2(net)?.len(),
    })
}
