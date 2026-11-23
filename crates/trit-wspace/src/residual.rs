//! 低秩残差侧车：把三值化误差 `R = W − s·T` 的 top-r 奇异三元组留成 f32 修正。
//!
//! 三值主干只存 `{-1, 0, +1}` 与 scale；本模块把误差矩阵 `R` 分解成
//! `R ≈ U_r Σ_r V_rᵀ`，推理时 `y += U_r (Σ_r V_rᵀ x)`。存储是
//! `rank · (rows + cols)` 个 f32：`rank = min(rows, cols)` 时等价于完整 f32 线性层，
//! 其余情况下是「三值主干 + 少量 f32」的侧车。
//!
//! 分解走对称 Gram 矩阵的 Jacobi 全分解：`cols <= rows` 时分解 `RᵀR`（`cols` 维），
//! 否则分解 `R Rᵀ`（`rows` 维），维度总是 `min(rows, cols)`。全分解保证基正交、
//! 结果确定；更小的 `rank` 是同一组列的**前缀**，所以不会出现「rank 增大反而更差」
//! 的非单调伪影（幂迭代 + 一次正交化在谱比接近 1 时做不到这一点）。

use crate::error::{Error, Result};

/// Jacobi 全分解的固定扫描轮数；384 维在这个轮数下已收敛到 f64 精度。
const SWEEPS: usize = 20;

/// 三值线性层残差的 top-r SVD 侧车。
///
/// `u` 是 `rows × rank`、`v` 是 `cols × rank`，修正量为
/// `Σ_k u[row, k] · (v[:, k] · x)`；两种取向下都只有一侧是正交基，
/// 另一侧已经吸收奇异值，因此乘积恒为 `R` 的 rank 截断。
#[derive(Clone, Debug)]
pub struct LowRankResidual {
    rows: usize,
    cols: usize,
    capacity: usize,
    rank: usize,
    /// `rows × capacity`（行主序），列是 `R·v_k` 或 `u_k`。
    u: Vec<f32>,
    /// `cols × capacity`（行主序），列是 `v_k` 或 `Rᵀ·u_k`。
    v: Vec<f32>,
}

impl LowRankResidual {
    /// 由三值权重、`scale` 与 `rank` 构造侧车；`rank == 0` 返回 `None`。
    ///
    /// `scales` 按 `(row * groups + g)` 布局，`groups = ceil(cols / group)`；
    /// `group >= cols` 表示逐行 scale。`rank` 超过 `min(rows, cols)` 时按上限截断
    /// （此时等价于完整 f32 权重）。
    pub fn build(
        weight: &[f32],
        rows: usize,
        cols: usize,
        trits: &[i8],
        scales: &[f32],
        group: usize,
        rank: usize,
    ) -> Result<Option<Self>> {
        if rows == 0 || cols == 0 {
            return Err(Error::EmptyLayer { rows, cols });
        }
        if weight.len() != rows * cols {
            return Err(Error::Shape {
                what: "weight",
                expected: rows * cols,
                got: weight.len(),
            });
        }
        if trits.len() != rows * cols {
            return Err(Error::Shape {
                what: "trits",
                expected: rows * cols,
                got: trits.len(),
            });
        }
        let group = group.clamp(1, cols);
        let groups = cols.div_ceil(group);
        if scales.len() != rows * groups {
            return Err(Error::Shape {
                what: "scales",
                expected: rows * groups,
                got: scales.len(),
            });
        }
        let rank = rank.min(rows.min(cols));
        if rank == 0 {
            return Ok(None);
        }
        let mut residual = vec![0.0f64; rows * cols];
        for row in 0..rows {
            for col in 0..cols {
                let scale = scales[row * groups + col / group];
                residual[row * cols + col] = f64::from(weight[row * cols + col])
                    - trit_value(trits[row * cols + col]) * f64::from(scale);
            }
        }
        let (u, v) = if cols <= rows {
            let gram = gram_left(&residual, rows, cols);
            let (_, vectors) = symmetric_eigen(&gram, cols, SWEEPS);
            let mut u = vec![0.0f32; rows * rank];
            for row in 0..rows {
                for k in 0..rank {
                    let mut acc = 0.0f64;
                    for col in 0..cols {
                        acc += residual[row * cols + col] * vectors[col * cols + k];
                    }
                    u[row * rank + k] = acc as f32;
                }
            }
            let mut v = vec![0.0f32; cols * rank];
            for col in 0..cols {
                for k in 0..rank {
                    v[col * rank + k] = vectors[col * cols + k] as f32;
                }
            }
            (u, v)
        } else {
            let gram = gram_right(&residual, rows, cols);
            let (_, vectors) = symmetric_eigen(&gram, rows, SWEEPS);
            let mut u = vec![0.0f32; rows * rank];
            for row in 0..rows {
                for k in 0..rank {
                    u[row * rank + k] = vectors[row * rows + k] as f32;
                }
            }
            let mut v = vec![0.0f32; cols * rank];
            for col in 0..cols {
                for k in 0..rank {
                    let mut acc = 0.0f64;
                    for row in 0..rows {
                        acc += residual[row * cols + col] * vectors[row * rows + k];
                    }
                    v[col * rank + k] = acc as f32;
                }
            }
            (u, v)
        };
        Ok(Some(Self {
            rows,
            cols,
            capacity: rank,
            rank,
            u,
            v,
        }))
    }

    /// 当前生效的 rank（`set_rank` 只能在前缀内收缩）。
    pub fn rank(&self) -> usize {
        self.rank
    }

    /// 构造时的最大 rank。
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 行数。
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// 列数。
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// 侧车占用的 f32 个数：`rank · (rows + cols)`。
    pub fn params(&self) -> usize {
        self.rank * (self.rows + self.cols)
    }

    /// 把生效 rank 收缩到 `rank`；只能变小，列顺序不变，因此读数关于 rank 单调。
    pub fn set_rank(&mut self, rank: usize) -> Result<()> {
        if rank > self.capacity {
            return Err(Error::Shape {
                what: "sidecar rank",
                expected: self.capacity,
                got: rank,
            });
        }
        self.rank = rank;
        Ok(())
    }

    /// 把一个 token 的输入投影到 `rank` 个侧车坐标：`p_k = v[:, k] · x`。
    pub fn projections(&self, input: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0f32; self.rank];
        for (k, slot) in out.iter_mut().enumerate() {
            let mut acc = 0.0f64;
            for (col, &value) in input.iter().enumerate().take(self.cols) {
                acc += f64::from(self.v[col * self.capacity + k]) * f64::from(value);
            }
            *slot = acc as f32;
        }
        out
    }

    /// 第 `row` 行的修正量：`Σ_k u[row, k] · projections[k]`。
    pub fn correction(&self, row: usize, projections: &[f32]) -> f64 {
        let line = &self.u[row * self.capacity..row * self.capacity + self.rank];
        line.iter()
            .zip(projections)
            .map(|(&u, &p)| f64::from(u) * f64::from(p))
            .sum()
    }

    /// 便捷入口：直接从输入算第 `row` 行的修正量（内部会做一次投影）。
    pub fn correction_direct(&self, row: usize, input: &[f32]) -> f64 {
        self.correction(row, &self.projections(input))
    }
}

/// 三值只允许 `-1/0/+1`；训练/导入侧理论上不会给出别的值，这里仍然挡住。
fn trit_value(trit: i8) -> f64 {
    debug_assert!((-1..=1).contains(&trit), "trit 越界：{trit}");
    f64::from(trit)
}

/// `RᵀR`（`cols × cols`，只填一半再对称化）。
fn gram_left(residual: &[f64], rows: usize, cols: usize) -> Vec<f64> {
    let mut gram = vec![0.0f64; cols * cols];
    for row in 0..rows {
        let line = &residual[row * cols..(row + 1) * cols];
        for a in 0..cols {
            let left = line[a];
            if left == 0.0 {
                continue;
            }
            for b in a..cols {
                let value = left * line[b];
                gram[a * cols + b] += value;
                if a != b {
                    gram[b * cols + a] += value;
                }
            }
        }
    }
    gram
}

/// `R Rᵀ`（`rows × rows`）。
fn gram_right(residual: &[f64], rows: usize, cols: usize) -> Vec<f64> {
    let mut gram = vec![0.0f64; rows * rows];
    for a in 0..rows {
        let left = &residual[a * cols..(a + 1) * cols];
        for b in a..rows {
            let right = &residual[b * cols..(b + 1) * cols];
            let value = left.iter().zip(right).map(|(&x, &y)| x * y).sum::<f64>();
            gram[a * rows + b] += value;
            if a != b {
                gram[b * rows + a] += value;
            }
        }
    }
    gram
}

/// 对称矩阵的 Jacobi 全分解，返回按特征值降序的 `(特征值, 特征向量)`；
/// 特征向量按列存：第 `k` 个向量是 `vectors[.., k]`（即 `vectors[i * n + k]`）。
fn symmetric_eigen(matrix: &[f64], n: usize, sweeps: usize) -> (Vec<f64>, Vec<f64>) {
    let mut m = matrix.to_vec();
    let mut v = vec![0.0f64; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..sweeps {
        let mut off = 0.0;
        let mut diag = 0.0;
        for i in 0..n {
            diag += m[i * n + i] * m[i * n + i];
            for j in (i + 1)..n {
                off += m[i * n + j] * m[i * n + j];
            }
        }
        if off <= 1e-24 * diag.max(f64::MIN_POSITIVE) {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = m[p * n + q];
                if apq.abs() < 1e-18 * diag.sqrt().max(f64::MIN_POSITIVE) {
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
    let mut order: Vec<(f64, usize)> = (0..n).map(|j| (m[j * n + j], j)).collect();
    order.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut values = vec![0.0f64; n];
    let mut vectors = vec![0.0f64; n * n];
    for (k, &(value, column)) in order.iter().enumerate() {
        values[k] = value;
        for i in 0..n {
            vectors[i * n + k] = v[i * n + column];
        }
    }
    (values, vectors)
}

#[cfg(test)]
mod tests {
    use super::symmetric_eigen;

    #[test]
    fn eigen_vectors_are_orthonormal() {
        let n = 5;
        let mut m = vec![0.0f64; n * n];
        for i in 0..n {
            for j in 0..n {
                m[i * n + j] = if i == j { i as f64 + 1.0 } else { 0.5 };
            }
        }
        let (values, vectors) = symmetric_eigen(&m, n, 20);
        for a in 0..n {
            for b in 0..n {
                let dot: f64 = (0..n)
                    .map(|i| vectors[i * n + a] * vectors[i * n + b])
                    .sum();
                let want = if a == b { 1.0 } else { 0.0 };
                assert!((dot - want).abs() < 1e-10, "列 {a}/{b} 不正交：{dot}");
            }
        }
        for k in 1..n {
            assert!(values[k - 1] >= values[k], "特征值必须降序");
        }
    }
}
