//! 三值权重的 GPTQ 式逐列补偿（OBS 近似，免训练）。
//!
//! [`absmean`] 是原始逐行 absmean 三值化；[`quantize`] 在此之上用校准集的
//! `H = XᵀX`（`X` 是该层的 f32 输入）逐列量化并补偿尚未处理的列：
//!
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
    check_shape(weight, rows, cols, "weight")?;
    check_shape(hessian, cols, cols, "hessian")?;
    if rows == 0 || cols == 0 {
        return Err(Error::EmptyLayer { rows, cols });
    }
    let inverse = damped_inverse(hessian, cols);
    let scales: Vec<f32> = (0..rows)
        .map(|row| {
            let line = &weight[row * cols..(row + 1) * cols];
            let mean = line.iter().map(|value| value.abs()).sum::<f32>() / cols as f32;
            if mean == 0.0 { 1.0 } else { mean }
        })
        .collect();
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
        for row in 0..rows {
            let scale = scales[row];
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

/// `H⁻¹`（1% 对角阻尼，GPTQ 惯例），f64 部分主元 Gauss-Jordan。
fn damped_inverse(hessian: &[f32], cols: usize) -> Vec<f64> {
    let width = 2 * cols;
    let mut aug = vec![0.0f64; cols * width];
    let mean = (0..cols)
        .map(|index| f64::from(hessian[index * cols + index]))
        .sum::<f64>()
        / cols as f64;
    let damp = if mean.is_finite() && mean > 0.0 {
        mean * 0.01
    } else {
        1e-6
    };
    for row in 0..cols {
        for col in 0..cols {
            aug[row * width + col] = f64::from(hessian[row * cols + col]);
        }
        aug[row * width + cols + row] = 1.0;
        aug[row * width + row] += damp;
    }
    for col in 0..cols {
        let mut pivot = col;
        for row in col + 1..cols {
            if aug[row * width + col].abs() > aug[pivot * width + col].abs() {
                pivot = row;
            }
        }
        if pivot != col {
            for offset in 0..width {
                aug.swap(col * width + offset, pivot * width + offset);
            }
        }
        let divisor = aug[col * width + col];
        if divisor == 0.0 || !divisor.is_finite() {
            continue;
        }
        for offset in 0..width {
            aug[col * width + offset] /= divisor;
        }
        for row in 0..cols {
            if row == col {
                continue;
            }
            let factor = aug[row * width + col];
            if factor == 0.0 {
                continue;
            }
            for offset in 0..width {
                aug[row * width + offset] -= factor * aug[col * width + offset];
            }
        }
    }
    let mut inverse = vec![0.0f64; cols * cols];
    for row in 0..cols {
        for col in 0..cols {
            inverse[row * cols + col] = aug[row * width + cols + col];
        }
    }
    inverse
}

fn check_shape(values: &[f32], rows: usize, cols: usize, what: &'static str) -> Result<()> {
    let expected = rows.checked_mul(cols).ok_or(Error::Shape {
        what,
        expected: usize::MAX,
        got: values.len(),
    })?;
    if values.len() != expected {
        return Err(Error::Shape {
            what,
            expected,
            got: values.len(),
        });
    }
    Ok(())
}
