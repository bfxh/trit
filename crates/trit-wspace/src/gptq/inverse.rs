//! `H⁻¹` 与形状校验（从 `gptq.rs` 拆出，守住行数预算）。
use crate::error::{Error, Result};

/// `H⁻¹`（1% 对角阻尼，GPTQ 惯例），f64 部分主元 Gauss-Jordan。
pub(super) fn damped_inverse(hessian: &[f32], cols: usize) -> Vec<f64> {
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

pub(super) fn check_shape(
    values: &[f32],
    rows: usize,
    cols: usize,
    what: &'static str,
) -> Result<()> {
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
