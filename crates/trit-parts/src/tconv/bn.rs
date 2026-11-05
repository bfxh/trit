/// 批归一化（单图训练 ⇒ 按通道在**空间维**归一化），带 running stats 供评测。
#[derive(Clone)]
pub struct Bn {
    pub gamma: Vec<f32>,
    pub beta: Vec<f32>,
    pub mean: Vec<f32>,
    pub var: Vec<f32>,
    pub momentum: f32,
    /// 前向实际用的批统计（反向必须用同一份）。
    batch_mean: Vec<f32>,
    batch_var: Vec<f32>,
    // Adam 状态
    mg: Vec<f32>,
    vg: Vec<f32>,
    mb: Vec<f32>,
    vb: Vec<f32>,
    t: u32,
}

impl Bn {
    pub fn new(c: usize) -> Self {
        Bn {
            gamma: vec![1.0; c],
            beta: vec![0.0; c],
            mean: vec![0.0; c],
            var: vec![1.0; c],
            momentum: 0.9,
            batch_mean: vec![0.0; c],
            batch_var: vec![1.0; c],
            mg: vec![0.0; c],
            vg: vec![0.0; c],
            mb: vec![0.0; c],
            vb: vec![0.0; c],
            t: 0,
        }
    }

    /// 前向：返回 `(γ·x̂+β, x̂)`；`train` 时更新 running stats。
    pub fn forward(
        &mut self,
        x: &[Vec<f32>],
        n: usize,
        train: bool,
    ) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
        let mut xh = vec![vec![0.0f32; n]; x.len()];
        let mut y = vec![vec![0.0f32; n]; x.len()];
        for c in 0..x.len() {
            let mu = x[c].iter().sum::<f32>() / n as f32;
            let var = x[c].iter().map(|v| (v - mu) * (v - mu)).sum::<f32>() / n as f32;
            if train {
                self.mean[c] = self.momentum * self.mean[c] + (1.0 - self.momentum) * mu;
                self.var[c] = self.momentum * self.var[c] + (1.0 - self.momentum) * var;
            }
            self.batch_mean[c] = mu;
            self.batch_var[c] = var;
            let (m, v) = if train {
                (mu, var)
            } else {
                (self.mean[c], self.var[c])
            };
            let inv = 1.0 / (v + 1e-5).sqrt();
            for i in 0..n {
                let z = (x[c][i] - m) * inv;
                xh[c][i] = z;
                y[c][i] = self.gamma[c] * z + self.beta[c];
            }
        }
        (y, xh)
    }

    /// 反向：`xhat` 是归一化后的值（不含 γ/β）。返回 `(dx, dγ, dβ)`，不更新。
    pub fn backward(
        &self,
        xhat: &[Vec<f32>],
        dout: &[Vec<f32>],
        n: usize,
    ) -> (Vec<Vec<f32>>, Vec<f32>, Vec<f32>) {
        let mut dx = vec![vec![0.0f32; n]; dout.len()];
        let mut dgs = Vec::with_capacity(dout.len());
        let mut dbs = Vec::with_capacity(dout.len());
        for c in 0..dout.len() {
            let dgamma: f32 = dout[c].iter().zip(&xhat[c]).map(|(d, z)| d * z).sum();
            let dbeta: f32 = dout[c].iter().sum();
            let mean_d = dbeta / n as f32;
            let mean_dx: f32 = dout[c]
                .iter()
                .zip(&xhat[c])
                .map(|(d, z)| d * z)
                .sum::<f32>()
                / n as f32;
            // 用前向的**批**方差（不是 running var）
            let inv = 1.0 / (self.batch_var[c] + 1e-5).sqrt();
            for i in 0..n {
                dx[c][i] = self.gamma[c] * inv * (dout[c][i] - mean_d - xhat[c][i] * mean_dx);
            }
            dgs.push(dgamma);
            dbs.push(dbeta);
        }
        (dx, dgs, dbs)
    }

    /// 用累积梯度更新 γ/β（`adam` 同 Conv）。
    pub fn apply(&mut self, dg: &[f32], db: &[f32], lr: f32, adam: bool) {
        let c = self.gamma.len();
        if adam {
            self.t += 1;
            let (b1, b2, eps) = (0.9f32, 0.999f32, 1e-8f32);
            let c1 = 1.0 - b1.powi(self.t as i32);
            let c2 = 1.0 - b2.powi(self.t as i32);
            for i in 0..c {
                self.mg[i] = b1 * self.mg[i] + (1.0 - b1) * dg[i];
                self.vg[i] = b2 * self.vg[i] + (1.0 - b2) * dg[i] * dg[i];
                self.gamma[i] -= lr * (self.mg[i] / c1) / ((self.vg[i] / c2).sqrt() + eps);
                self.mb[i] = b1 * self.mb[i] + (1.0 - b1) * db[i];
                self.vb[i] = b2 * self.vb[i] + (1.0 - b2) * db[i] * db[i];
                self.beta[i] -= lr * (self.mb[i] / c1) / ((self.vb[i] / c2).sqrt() + eps);
            }
        } else {
            for i in 0..c {
                self.gamma[i] -= lr * dg[i];
                self.beta[i] -= lr * db[i];
            }
        }
    }
}

/// 辅助分类头：对隐特征做全局平均池化 → 线性 → 类别 logits。
/// 目的：给「逐点回归」补一条**对类别有可辨识性**的损失（FiLM 单独用会被忽略）。
pub struct AuxCls {
    pub w: Vec<f32>,
    pub b: Vec<f32>,
    pub nk: usize,
    pub c: usize,
}

impl AuxCls {
    pub fn new(nk: usize, c: usize) -> Self {
        AuxCls {
            w: vec![0.0; nk * c],
            b: vec![0.0; nk],
            nk,
            c,
        }
    }
    pub(crate) fn logits(&self, pool: &[f32]) -> Vec<f32> {
        (0..self.nk)
            .map(|k| {
                self.b[k]
                    + (0..self.c)
                        .map(|i| self.w[k * self.c + i] * pool[i])
                        .sum::<f32>()
            })
            .collect()
    }
}

/// FiLM：按类别对隐层做逐通道缩放/平移（`y = g[c]·y + b[c]`）。
/// 行数 = 类别数 + 1，最后一行为**空类**（条件丢弃时用）。
pub struct Film {
    pub g: Vec<f32>,
    pub b: Vec<f32>,
    pub nk: usize,
    pub c: usize,
}

impl Film {
    pub fn new(nk: usize, c: usize) -> Self {
        let rows = nk + 1;
        Film {
            g: vec![1.0; rows * c],
            b: vec![0.0; rows * c],
            nk,
            c,
        }
    }
    pub fn row(&self, class: Option<usize>) -> usize {
        class.filter(|&k| k < self.nk).unwrap_or(self.nk)
    }
    pub fn gamma(&self, class: Option<usize>) -> &[f32] {
        let r = self.row(class);
        &self.g[r * self.c..(r + 1) * self.c]
    }
    pub fn beta(&self, class: Option<usize>) -> &[f32] {
        let r = self.row(class);
        &self.b[r * self.c..(r + 1) * self.c]
    }
}
