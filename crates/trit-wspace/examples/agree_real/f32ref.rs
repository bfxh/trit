use trit_wspace::Trit;
use trit_wspace::safetensors::Weights;

fn sign(value: f64) -> Trit {
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

    pub fn sign_of(&self, x: &[f32]) -> Trit {
        let mut xk = vec![0.0f64; self.k];
        for (i, slot) in xk.iter_mut().enumerate() {
            let u = &self.proj[i * self.dim..(i + 1) * self.dim];
            *slot = u.iter().zip(x).map(|(a, b)| a * f64::from(*b)).sum();
        }
        let mut score = 0.0f64;
        for row in 0..self.rows {
            let w = &self.wk[row * self.k..(row + 1) * self.k];
            let v: f64 = w.iter().zip(&xk).map(|(a, b)| a * b).sum();
            score += f64::from(self.readout[row]) * v;
        }
        sign(score)
    }
}

/// 两条路径相对同一个 f32 基准的一致率：`(f32 投影, 三值投影)`。
///
/// 前者只剩投影损失；两者之差就是「三值化权重 + 三值化输入」的代价。
///
/// 判据是线性的（`score = w_eff·x`，`w_eff = queryᵀreadout`），所以投影到第 0 维
/// `w_eff` 后做 f32 点积是**精确**重写，`f32` 读数应恒为 1.0；它同时充当实现自检。
pub fn compare(
    base: &[Trit],
    raw: &[&[f32]],
    tern: &[Vec<Trit>],
    projected: &Projected,
) -> (f64, f64) {
    let n = base.len().min(raw.len()).min(tern.len());
    let mut f32_same = 0usize;
    let mut trit_same = 0usize;
    for i in 0..n {
        if projected.sign_of(raw[i]) == base[i] {
            f32_same += 1;
        }
        if tern[i].as_slice() == [base[i]] {
            trit_same += 1;
        }
    }
    let n = n.max(1) as f64;
    (f32_same as f64 / n, trit_same as f64 / n)
}

/// 三值输入投影到基的前 `k` 维再取符号（原先内联在 `main` 里）。
pub fn reduce(input: &[Trit], sub: &[Vec<f64>]) -> Vec<Trit> {
    sub.iter()
        .map(|u| {
            let mut acc = 0.0f64;
            for (a, value) in u.iter().zip(input) {
                acc += a * f64::from(value.to_i8());
            }
            sign(acc)
        })
        .collect()
}

/// 词表前 `samples` 行（**未**三值化的原始 f32）切片。
pub fn rows(embeddings: &Weights, dim: usize, samples: usize) -> Vec<&[f32]> {
    (0..samples)
        .map(|row| &embeddings.values[row * dim..(row + 1) * dim])
        .collect()
}

/// 每行上的未压缩 f32 判据输出，作为两条降维路径的共同基准。
pub fn baseline(query: &Weights, readout: &[f32], raw: &[&[f32]]) -> Vec<Trit> {
    raw.iter().map(|x| reference(query, readout, x)).collect()
}
