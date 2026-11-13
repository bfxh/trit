//! agree_real 的数学部分：二阶矩、Jacobi 特征分解、Gram-Schmidt 正交化。
//!
//! 从 `main.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use trit_wspace::safetensors::Weights;

pub fn second_moment(embeddings: &Weights, dim: usize, samples: usize) -> Vec<f64> {
    let mut mom = vec![0.0f64; dim * dim];
    for row in 0..samples.min(embeddings.rows) {
        let x = &embeddings.values[row * dim..(row + 1) * dim];
        for (i, &xi) in x.iter().enumerate() {
            let xi = f64::from(xi);
            for (j, &xj) in x.iter().enumerate() {
                mom[i * dim + j] += xi * f64::from(xj);
            }
        }
    }
    mom
}

pub fn jacobi_eigen(matrix: &[f64], n: usize, sweeps: usize) -> Vec<Vec<f64>> {
    let mut m = matrix.to_vec();
    let mut v = vec![0.0f64; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..sweeps {
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = m[p * n + q];
                if apq.abs() < 1e-18 {
                    continue;
                }
                let theta = (m[q * n + q] - m[p * n + p]) / (2.0 * apq);
                let t = if theta >= 0.0 {
                    1.0 / (theta + (theta * theta + 1.0).sqrt())
                } else {
                    -1.0 / (-theta + (theta * theta + 1.0).sqrt())
                };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let mkp = m[k * n + p];
                    let mkq = m[k * n + q];
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let mpk = m[p * n + k];
                    let mqk = m[q * n + k];
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut pairs: Vec<(f64, Vec<f64>)> = (0..n)
        .map(|j| {
            (
                m[j * n + j],
                (0..n).map(|i| v[i * n + j]).collect::<Vec<f64>>(),
            )
        })
        .collect();
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    pairs.into_iter().map(|(_, vector)| vector).collect()
}

pub fn orthonormalize(v: &mut [f64], basis: &[Vec<f64>]) -> f64 {
    for b in basis {
        let proj: f64 = v.iter().zip(b).map(|(a, c)| a * c).sum();
        for (slot, c) in v.iter_mut().zip(b) {
            *slot -= proj * c;
        }
    }
    let norm = v.iter().map(|a| a * a).sum::<f64>().sqrt();
    if norm > 0.0 {
        for a in v.iter_mut() {
            *a /= norm;
        }
    }
    norm
}
