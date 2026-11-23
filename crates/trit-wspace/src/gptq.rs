//! 三值权重的 GPTQ 式逐列补偿（OBS 近似，免训练）。
//! [`absmean`] 是原始逐行 absmean 三值化；[`quantize`] 在此之上用校准集的
//! `H = XᵀX`（`X` 是该层的 f32 输入）逐列量化并补偿尚未处理的列：
//! ```text
//! q_j = clamp(round(w_j / s), -1, 1)
//! e_j = (w_j - s·q_j) / [H⁻¹]_jj
//! w_k -= e_j · [H⁻¹]_jk        （k 尚未量化）
//! ```
//!
//! 列顺序按 `diag(H)` 降序（GPTQ 的 act-order），`H` 加 1% 对角阻尼保证可逆。
//! 补偿系数必须用 `H⁻¹` 而不是 `H`：两者非对角元素常反号，用 `H` 会把误差推向
//! **增大** 的方向（实测 H 加权误差从 0.27 涨到 0.86，端到端 STS 从 0.733 掉到 0.492）。

use crate::error::{Error, Result};
mod inverse;
use inverse::{check_shape, damped_inverse};

/// absmean 逐行三值化：`scale = mean(|w_row|)`，`q = clamp(round(w/scale), -1, 1)`。
///
/// 全零行的 `scale` 记为 `1.0`（与 [`crate::safetensors::Weights::to_layer_with_bias`]
/// 同口径），这样调用方不必再做除零保护。
pub fn absmean(weight: &[f32], rows: usize, cols: usize) -> Result<(Vec<i8>, Vec<f32>)> {
    check_shape(weight, rows, cols, "weight")?;
    let mut trits = Vec::with_capacity(rows * cols);
    let mut scales = Vec::with_capacity(rows);
    for row in 0..rows {
        let quantized = trit_learn::ternarize(&weight[row * cols..(row + 1) * cols]);
        trits.extend_from_slice(&quantized.q);
        scales.push(if quantized.scale == 0.0 {
            1.0
        } else {
            quantized.scale
        });
    }
    Ok((trits, scales))
}

/// GPTQ 逐列补偿三值化：`hessian` 是校准集输入的 `XᵀX`（`cols × cols`，行主序）。
///
/// 返回 `(trits, scales)`；`scales` 仍是逐行 absmean，与 [`absmean`] 同口径，
/// 因此两边的 `trits` 只差补偿/重排带来的少数列，可直接比较。
pub fn quantize(
    weight: &[f32],
    rows: usize,
    cols: usize,
    hessian: &[f32],
) -> Result<(Vec<i8>, Vec<f32>)> {
    quantize_grouped(weight, rows, cols, hessian, cols)
}

/// per-group scale 版 GPTQ：每行按 `group` 列切块，每块用自己的 absmean scale。
///
/// `scales` 布局是 `(row * groups + g)`，`groups = ceil(cols / group)`；`group == cols`
/// 时退化为 [`quantize`]（每行一个 scale）。`cols < group` 按 `cols` 处理。
pub fn quantize_grouped(
    weight: &[f32],
    rows: usize,
    cols: usize,
    hessian: &[f32],
    group: usize,
) -> Result<(Vec<i8>, Vec<f32>)> {
    check_shape(weight, rows, cols, "weight")?;
    check_shape(hessian, cols, cols, "hessian")?;
    if rows == 0 || cols == 0 {
        return Err(Error::EmptyLayer { rows, cols });
    }
    let group = group.clamp(1, cols);
    let groups = cols.div_ceil(group);
    let inverse = damped_inverse(hessian, cols);
    let mut scales = vec![0.0f32; rows * groups];
    for row in 0..rows {
        for chunk in 0..groups {
            let start = chunk * group;
            let end = (start + group).min(cols);
            let sum = weight[row * cols + start..row * cols + end]
                .iter()
                .map(|value| value.abs())
                .sum::<f32>();
            let mean = sum / (end - start) as f32;
            scales[row * groups + chunk] = if mean == 0.0 { 1.0 } else { mean };
        }
    }
    let mut working = weight.to_vec();
    let mut trits = vec![0i8; rows * cols];
    let mut order: Vec<usize> = (0..cols).collect();
    order.sort_by(|&a, &b| {
        hessian[b * cols + b]
            .partial_cmp(&hessian[a * cols + a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (position, &column) in order.iter().enumerate() {
        let diagonal = inverse[column * cols + column];
        let compensated = diagonal.abs() > 1e-12;
        let chunk = column / group;
        for row in 0..rows {
            let scale = scales[row * groups + chunk];
            let value = working[row * cols + column];
            let quantized = (value / scale).round().clamp(-1.0, 1.0);
            trits[row * cols + column] = quantized as i8;
            if !compensated {
                continue;
            }
            let error = f64::from(value - scale * quantized) / diagonal;
            if error == 0.0 {
                continue;
            }
            for &next in &order[position + 1..] {
                let delta = error * inverse[column * cols + next];
                working[row * cols + next] -= delta as f32;
            }
        }
    }
    Ok((trits, scales))
}

mod bits;
pub use bits::quantize_grouped_bits;
