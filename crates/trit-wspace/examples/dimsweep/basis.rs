//! dimsweep 的数学部分：三值化、Jacobi/PCA、判据方向与降维网络。
//!
//! 从 `main.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use trit_wspace::safetensors::Weights;
use trit_wspace::{Layer, Net, Oracle, Trit};

/// 确定性 xorshift64*。
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        let state = seed ^ 0x9E37_79B9_7F4A_7C15;
        Self(if state == 0 { 1 } else { state })
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

pub fn sign_of(score: f32) -> Trit {
    if score > 0.0 {
        Trit::Pos
    } else if score < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

/// 一行 f32 按 absmean 三值化（与 `Weights::to_layer` 同一实现）。
pub fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize row");
    (0..values.len())
        .map(|col| layer.trit(0, col).expect("trit"))
        .collect()
}

pub fn trit_row_i8(values: &[f32]) -> Vec<i8> {
    trit_row(values).iter().map(|t| t.to_i8()).collect()
}

/// 读出层：1×dim 的三值线性层。
pub fn readout_layer(readout: &[f32]) -> Layer {
    Layer::new(1, readout.len(), &trit_row_i8(readout), &[0]).expect("readout layer")
}

/// f32 教师前向：`query × x`。
pub fn teacher_forward(query: &Weights, x: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; query.rows];
    for (row, slot) in out.iter_mut().enumerate() {
        let weights = &query.values[row * query.cols..(row + 1) * query.cols];
        *slot = weights
            .iter()
            .zip(x)
            .map(|(weight, value)| weight * value)
            .sum();
    }
    out
}

pub fn dot(readout: &[f32], values: &[f32]) -> f32 {
    readout.iter().zip(values).map(|(r, v)| r * v).sum()
}

/// 一个样本：原始 384 维输入 + 全维 f32 教师的标签。
#[derive(Clone)]
pub struct Sample {
    pub x: Vec<f32>,
    pub teacher: Trit,
}

pub fn full_student(query: &Weights, readout: &[f32]) -> Net {
    Net::new(vec![
        query.to_layer().expect("ternary query"),
        readout_layer(readout),
    ])
    .expect("student net")
}

pub fn sample_dataset(
    query: &Weights,
    embeddings: &Weights,
    readout: &[f32],
    budget: usize,
    seed: u64,
) -> Vec<Sample> {
    let dim = query.cols;
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(budget);
    while out.len() < budget {
        let row = (rng.next() % embeddings.rows as u64) as usize;
        let x = embeddings.values[row * dim..(row + 1) * dim].to_vec();
        let teacher = sign_of(dot(readout, &teacher_forward(query, &x)));
        if teacher == Trit::Zero {
            continue;
        }
        out.push(Sample { x, teacher });
    }
    out
}

/// 真实词向量行的二阶矩 `M = E[x x^T]`（dim×dim，行主序 f64）。
///
/// 不去均值：`absmean` 三值化本来就不中心化，且这样降维层不需要常数偏置项
/// （`w·x ≈ (U^T w)·(U^T x)`），三值偏置不会吃掉一个 f32 常数。
pub fn second_moment(embeddings: &Weights, dim: usize, samples: usize, seed: u64) -> Vec<f64> {
    let mut rng = Rng::new(seed ^ 0x5DEE_CE66);
    let mut mom = vec![0.0f64; dim * dim];
    for _ in 0..samples {
        let row = (rng.next() % embeddings.rows as u64) as usize;
        let x = &embeddings.values[row * dim..(row + 1) * dim];
        for (i, &xi) in x.iter().enumerate() {
            let xi = f64::from(xi);
            for (j, &xj) in x.iter().enumerate() {
                mom[i * dim + j] += xi * f64::from(xj);
            }
        }
    }
    let scale = 1.0 / samples as f64;
    for value in &mut mom {
        *value *= scale;
    }
    mom
}

/// Jacobi 特征分解（对称矩阵）：返回按特征值降序的 (特征值, 方向)。
///
/// 384x384 的完整分解在这个规模上很快，且基严格正交：不会出现「k 增大反而更差」
/// 这种非单调伪影（幂迭代 + 一次 Gram-Schmidt 在谱比接近 1 时做不到）。
pub fn jacobi_eigen(matrix: &[f64], n: usize, sweeps: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut m = matrix.to_vec();
    let mut v = vec![0.0f64; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..sweeps {
        let mut off = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                off += m[i * n + j] * m[i * n + j];
            }
        }
        if off < 1e-22 {
            break;
        }
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
    pairs.into_iter().unzip()
}

/// 对已有方向做 Gram-Schmidt 正交化并归一化，返回正交化之前的范数。
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

/// 判据方向 `w_eff = queryᵀ readout`：教师本身就是线性的
/// （`readout · (query·x) = w_eff · x`），所以它是「哪一维承载判据」的精确答案。
pub fn criterion_direction(query: &Weights, readout: &[f32], dim: usize) -> Vec<f64> {
    let mut w = vec![0.0f64; dim];
    for (row, r) in readout.iter().enumerate() {
        let rw = &query.values[row * dim..(row + 1) * dim];
        for (slot, value) in w.iter_mut().zip(rw) {
            *slot += f64::from(*r) * f64::from(*value);
        }
    }
    w
}

/// 监督基：判据方向打头，其余用与它正交的 PCA 方向补齐。
///
/// 返回 (基, 判据方向与 PCA 第一主方向的 |cos|)。后者直接回答「方差方向离判据方向多远」；
/// `pca` 模式下这个值恒为 0。
pub fn supervised_basis(
    query: &Weights,
    readout: &[f32],
    dim: usize,
    pca: &[Vec<f64>],
    k: usize,
) -> (Vec<Vec<f64>>, f64) {
    let mut first = criterion_direction(query, readout, dim);
    let norm = first.iter().map(|a| a * a).sum::<f64>().sqrt();
    if norm > 0.0 {
        for a in first.iter_mut() {
            *a /= norm;
        }
    }
    let cos = pca.first().map_or(0.0, |u| {
        first.iter().zip(u).map(|(a, b)| a * b).sum::<f64>().abs()
    });
    let mut basis = vec![first];
    for u in pca {
        if basis.len() >= k {
            break;
        }
        let mut w = u.clone();
        if orthonormalize(&mut w, &basis) > 1e-6 {
            basis.push(w);
        }
    }
    (basis, cos)
}

/// 前 `k` 个主方向（特征值降序）。
pub fn top_components(
    matrix: &[f64],
    dim: usize,
    k: usize,
    sweeps: usize,
) -> (Vec<Vec<f64>>, Vec<f64>) {
    let (values, vectors) = jacobi_eigen(matrix, dim, sweeps);
    let k = k.min(dim);
    (
        vectors.into_iter().take(k).collect(),
        values.into_iter().take(k).collect(),
    )
}

/// `x_k = U^T x`（`U` 的每一行是一个主方向）。
pub fn project(x: &[f32], comps: &[Vec<f64>]) -> Vec<f32> {
    comps
        .iter()
        .map(|u| u.iter().zip(x).map(|(a, b)| a * f64::from(*b)).sum::<f64>() as f32)
        .collect()
}

/// 降维学生：第一层 `rows × k`（`w_k[r][i] = sum_j U[i][j] w[r][j]`），读出层不变。
pub fn reduced_net(query: &Weights, readout: &[f32], dim: usize, comps: &[Vec<f64>]) -> Net {
    let k = comps.len();
    let base = query.to_layer().expect("ternary query");
    let bias: Vec<i8> = base.bias().iter().map(|t| t.to_i8()).collect();
    let mut trits = vec![0i8; query.rows * k];
    for row in 0..query.rows {
        let w = &query.values[row * dim..(row + 1) * dim];
        for (i, u) in comps.iter().enumerate() {
            let acc: f64 = u.iter().zip(w).map(|(a, b)| a * f64::from(*b)).sum();
            trits[row * k + i] = if acc > 0.0 {
                1
            } else if acc < 0.0 {
                -1
            } else {
                0
            };
        }
    }
    let first = Layer::new(query.rows, k, &trits, &bias).expect("reduced layer");
    Net::new(vec![first, readout_layer(readout)]).expect("reduced net")
}

/// 在给定子空间上构造 oracle（输入投影 + 三值化，标签沿用全维教师）。
pub fn oracle_of(samples: &[Sample], comps: &[Vec<f64>]) -> Oracle {
    let inputs: Vec<Vec<Trit>> = samples
        .iter()
        .map(|s| trit_row(&project(&s.x, comps)))
        .collect();
    let outputs: Vec<Vec<Trit>> = samples.iter().map(|s| vec![s.teacher]).collect();
    Oracle::new(inputs, outputs).expect("oracle")
}

/// 恒等基（`k == dim` 时就是原全维学生，用原输入）。
pub fn identity(dim: usize) -> Vec<Vec<f64>> {
    (0..dim)
        .map(|i| {
            let mut v = vec![0.0f64; dim];
            v[i] = 1.0;
            v
        })
        .collect()
}
