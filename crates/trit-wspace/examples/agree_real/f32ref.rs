use trit_wspace::Trit;
use trit_wspace::safetensors::Weights;

/// 取符号：`>0 / <0 / =0` → `Pos / Neg / Zero`。
pub fn sign(value: f64) -> Trit {
    if value > 0.0 {
        Trit::Pos
    } else if value < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum()
}

/// 未压缩 f32 判据 `sign(readout · (query·x))`：权重与输入都不三值化。
pub fn reference(query: &Weights, readout: &[f32], x: &[f32]) -> Trit {
    let dim = query.cols;
    let mut score = 0.0f64;
    for row in 0..query.rows {
        let v = dot(&query.values[row * dim..(row + 1) * dim], x);
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
    pub fn new(query: &Weights, readout: &[f32], sub: &[Vec<f64>]) -> Self {
        let dim = query.cols;
        let rows = query.rows;
        let k = sub.len();
        let proj: Vec<f64> = sub.iter().flatten().copied().collect();
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
    pub fn dims(&self) -> (usize, usize) {
        (self.dim, self.k)
    }

    /// 逐行的投影权重（`rows × k`，f32 值）。
    pub fn weights(&self) -> &[f64] {
        &self.wk
    }

    /// 把 f32 输入投影到 `k` 维。
    pub fn project_f32(&self, x: &[f32]) -> Vec<f64> {
        (0..self.k)
            .map(|i| {
                let u = &self.proj[i * self.dim..(i + 1) * self.dim];
                u.iter().zip(x).map(|(a, b)| a * f64::from(*b)).sum()
            })
            .collect()
    }

    /// 把三值输入投影到 `k` 维（`±1/0` 直接点积）。
    pub fn project_trit(&self, x: &[Trit]) -> Vec<f64> {
        (0..self.k)
            .map(|i| {
                let u = &self.proj[i * self.dim..(i + 1) * self.dim];
                u.iter().zip(x).map(|(a, t)| a * f64::from(t.to_i8())).sum()
            })
            .collect()
    }

    /// 用给定（可能被三值化的）权重与投影输入判定，readout 保持 f32。
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

/// 词表前 `samples` 行（**未**三值化的原始 f32）切片。
pub fn rows(embeddings: &Weights, dim: usize, samples: usize) -> Vec<&[f32]> {
    (0..samples)
        .map(|row| &embeddings.values[row * dim..(row + 1) * dim])
        .collect()
}

/// 每行上的未压缩 f32 判据输出，作为所有降维路径的共同基准。
pub fn baseline(query: &Weights, readout: &[f32], raw: &[&[f32]]) -> Vec<Trit> {
    raw.iter().map(|x| reference(query, readout, x)).collect()
}
