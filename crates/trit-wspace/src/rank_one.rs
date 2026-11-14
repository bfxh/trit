//! 秩一编辑（ROME 式）：把一条"事实"写进一层权重的一行。
//!
//! ## 原理
//!
//! ROME（Meng et al. 2022）的核心是把知识的写入表达成**秩一更新**：给定某个输入的表示
//! `x`（subject 最后一个 token 的 key 向量）与希望得到的输出 `target = W* x`，求最小范数
//! 的 `δ` 使 `(W + δ xᵀ / (xᵀx)) x = target`。解直接给出：
//!
//! ```text
//! δ = target − W x
//! W' = W + δ xᵀ / (xᵀx)
//! ```
//!
//! 这保证 `W' x = target` **精确成立**（在 f32 精度内），且只改 `W` 的一个方向。
//! 原论文还要用 key 的协方差 `C` 让编辑尽量不干扰其它输入（`δ = (C⁻¹k*)/(k*ᵀC⁻¹k*)·(...)`）；
//! 本模块做**单点精确版**——先保证"这条能写进去"，再去谈干扰。
//!
//! ## 与三值空间的关系
//!
//! 秩一更新是**稠密**的（`δ xᵀ` 会动到整行），而三值化只能保留每行的大致方向。
//! 所以"三值化之后这条编辑还剩多少"是一个真问题——[`edit_error`] 就是用来量的。
//!
//! 用法：
//! ```text
//! rank_one_update(&mut weight, rows, cols, &x, &target)?;   // 写入
//! let err = edit_error(&weight, rows, cols, &x, &target);    // 0.0 表示写准了
//! ```

use crate::error::{Error, Result};

/// 行优先矩阵乘向量：`y = W x`。
#[must_use]
pub fn apply(weight: &[f32], rows: usize, cols: usize, x: &[f32]) -> Vec<f32> {
    (0..rows)
        .map(|row| {
            weight[row * cols..(row + 1) * cols]
                .iter()
                .zip(x)
                .map(|(w, v)| w * v)
                .sum()
        })
        .collect()
}

/// 最小范数秩一更新：让 `W x = target` 精确成立。
///
/// `weight` 行优先、形状 `rows × cols`。
pub fn rank_one_update(
    weight: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    target: &[f32],
) -> Result<()> {
    if rows == 0 || cols == 0 {
        return Err(Error::EmptyLayer { rows, cols });
    }
    if weight.len() != rows * cols {
        return Err(Error::Shape {
            what: "rank-one weight",
            expected: rows * cols,
            got: weight.len(),
        });
    }
    if x.len() != cols {
        return Err(Error::Shape {
            what: "rank-one input",
            expected: cols,
            got: x.len(),
        });
    }
    if target.len() != rows {
        return Err(Error::Shape {
            what: "rank-one target",
            expected: rows,
            got: target.len(),
        });
    }
    let norm: f32 = x.iter().map(|value| value * value).sum();
    if norm <= 0.0 || !norm.is_finite() {
        return Err(Error::Shape {
            what: "rank-one input norm",
            expected: 1,
            got: 0,
        });
    }
    for (row, want) in target.iter().enumerate() {
        let span = row * cols..(row + 1) * cols;
        let current: f32 = weight[span.clone()].iter().zip(x).map(|(w, v)| w * v).sum();
        let scale = (want - current) / norm;
        for (slot, value) in weight[span].iter_mut().zip(x) {
            *slot += scale * value;
        }
    }
    Ok(())
}

/// 编辑的相对 L2 误差：`‖W x − target‖ / ‖target‖`。`0.0` 表示写准了。
#[must_use]
pub fn edit_error(weight: &[f32], rows: usize, cols: usize, x: &[f32], target: &[f32]) -> f64 {
    let out = apply(weight, rows, cols, x);
    let diff: f64 = out
        .iter()
        .zip(target)
        .map(|(a, b)| {
            let delta = f64::from(*a) - f64::from(*b);
            delta * delta
        })
        .sum::<f64>()
        .sqrt();
    let base: f64 = target
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    if base == 0.0 { diff } else { diff / base }
}

/// 编辑方向与目标方向的余弦：`cos(W x, target)`。用于看三值化后方向是否还在。
#[must_use]
pub fn edit_cosine(weight: &[f32], rows: usize, cols: usize, x: &[f32], target: &[f32]) -> f64 {
    let out = apply(weight, rows, cols, x);
    let dot: f64 = out
        .iter()
        .zip(target)
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    let left: f64 = out
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    let right: f64 = target
        .iter()
        .map(|v| f64::from(*v) * f64::from(*v))
        .sum::<f64>()
        .sqrt();
    if left == 0.0 || right == 0.0 {
        0.0
    } else {
        dot / (left * right)
    }
}

/// 编辑改动了多少个**三值符号**：`W'` 与 `W` 逐元素符号不同的个数。
///
/// 秩一更新是稠密的，这个数通常接近整行——正是"为什么三值化会破坏它"的直观来源。
#[must_use]
pub fn changed_signs(before: &[f32], after: &[f32]) -> usize {
    before
        .iter()
        .zip(after)
        .filter(|(left, right)| left.signum() != right.signum())
        .count()
}
