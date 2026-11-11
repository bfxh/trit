//! 最小二乘 / 岭回归：正规方程 + Cholesky；附一份 f32 参考实现用于对拍。
//!
//! 约定：拟合**总是含截距**；岭回归只对斜率项加 `λ`（截距不惩罚）。
//! 奇异矩阵返回 [`NumError::Singular`]，不返回 `NaN` 或垃圾值。

use crate::NumError;
use crate::sums::sum_neumaier;

/// 拟合结果。
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// 截距。
    pub intercept: f64,
    /// 斜率（按列顺序）。
    pub coefficients: Vec<f64>,
    /// 残差平方和（用补偿求和）。
    pub residual_sum_squares: f64,
}

/// 拟合 `y ≈ intercept + Σ coefficients[j]·x[j]`（f64 路径）。
pub fn ols_f64(x: &[Vec<f64>], y: &[f64], lambda: f64) -> Result<Fit, NumError> {
    let (n, p) = validate(x, y, lambda)?;
    let q = p + 1;
    let mut xtx = vec![vec![0.0f64; q]; q];
    let mut xty = vec![0.0f64; q];
    for i in 0..n {
        let mut augmented = Vec::with_capacity(q);
        augmented.push(1.0);
        augmented.extend_from_slice(&x[i]);
        for j in 0..q {
            xty[j] += augmented[j] * y[i];
            for k in 0..q {
                xtx[j][k] += augmented[j] * augmented[k];
            }
        }
    }
    for (j, row) in xtx.iter_mut().enumerate().skip(1) {
        row[j] += lambda;
    }
    let mut rhs = xty;
    let beta = cholesky_solve(&mut xtx, &mut rhs)?;
    let residuals: Vec<f64> = (0..n)
        .map(|i| {
            let mut predicted = beta[0];
            for j in 0..p {
                predicted += beta[j + 1] * x[i][j];
            }
            y[i] - predicted
        })
        .collect();
    let squares: Vec<f64> = residuals.iter().map(|r| r * r).collect();
    Ok(Fit {
        intercept: beta[0],
        coefficients: beta[1..].to_vec(),
        residual_sum_squares: sum_neumaier(&squares),
    })
}

/// 同题的 **f32 参考实现**：同一算法、同一累加顺序，只降低精度。
///
/// 返回长度 `p + 1` 的向量（第 0 项是截距）。它与 [`ols_f64`] 的差就是精度对照。
pub fn ols_f32(x: &[Vec<f64>], y: &[f64], lambda: f64) -> Result<Vec<f32>, NumError> {
    let (n, p) = validate(x, y, lambda)?;
    let q = p + 1;
    let mut xtx = vec![vec![0f32; q]; q];
    let mut xty = vec![0f32; q];
    for i in 0..n {
        let mut augmented = Vec::with_capacity(q);
        augmented.push(1f32);
        for value in &x[i] {
            augmented.push(*value as f32);
        }
        let yi = y[i] as f32;
        for j in 0..q {
            xty[j] += augmented[j] * yi;
            for k in 0..q {
                xtx[j][k] += augmented[j] * augmented[k];
            }
        }
    }
    for (j, row) in xtx.iter_mut().enumerate().skip(1) {
        row[j] += lambda as f32;
    }
    let mut rhs = xty;
    cholesky_solve_f32(&mut xtx, &mut rhs)
}

/// 用拟合结果预测一行。
#[must_use]
pub fn predict(fit: &Fit, row: &[f64]) -> f64 {
    let products: Vec<f64> = fit
        .coefficients
        .iter()
        .zip(row)
        .map(|(coefficient, value)| coefficient * value)
        .collect();
    fit.intercept + sum_neumaier(&products)
}

fn validate(x: &[Vec<f64>], y: &[f64], lambda: f64) -> Result<(usize, usize), NumError> {
    if !lambda.is_finite() || lambda < 0.0 {
        return Err(NumError::BadLambda);
    }
    if x.len() != y.len() {
        return Err(NumError::LabelLength {
            x: x.len(),
            y: y.len(),
        });
    }
    if x.len() < 2 {
        return Err(NumError::NotEnoughSamples {
            samples: x.len(),
            needed: 2,
        });
    }
    let p = x.first().map(Vec::len).unwrap_or(0);
    if p == 0 {
        return Err(NumError::Dimension {
            expected: 1,
            actual: 0,
        });
    }
    for (i, row) in x.iter().enumerate() {
        if row.len() != p {
            return Err(NumError::Dimension {
                expected: p,
                actual: row.len(),
            });
        }
        for (j, value) in row.iter().enumerate() {
            if !value.is_finite() {
                return Err(NumError::NonFinite {
                    row: i,
                    col: j,
                    value: *value,
                });
            }
        }
    }
    for (i, value) in y.iter().enumerate() {
        if !value.is_finite() {
            return Err(NumError::NonFinite {
                row: i,
                col: 0,
                value: *value,
            });
        }
    }
    Ok((x.len(), p))
}

// 对称索引 + 固定累加顺序（确定性优先）；改成迭代器会让“同输入同结果”的阅读成本变高。
#[allow(clippy::needless_range_loop)]
fn cholesky_solve(a: &mut [Vec<f64>], b: &mut [f64]) -> Result<Vec<f64>, NumError> {
    let q = b.len();
    for i in 0..q {
        for j in 0..=i {
            let mut sum = a[i][j];
            for k in 0..j {
                sum -= a[i][k] * a[j][k];
            }
            if i == j {
                if !sum.is_finite() || sum <= 1e-12 {
                    return Err(NumError::Singular { pivot: sum });
                }
                a[i][j] = sum.sqrt();
            } else {
                a[i][j] = sum / a[j][j];
            }
        }
    }
    let mut z = vec![0.0f64; q];
    for i in 0..q {
        let mut sum = b[i];
        for k in 0..i {
            sum -= a[i][k] * z[k];
        }
        z[i] = sum / a[i][i];
    }
    let mut beta = vec![0.0f64; q];
    for i in (0..q).rev() {
        let mut sum = z[i];
        for k in (i + 1)..q {
            sum -= a[k][i] * beta[k];
        }
        beta[i] = sum / a[i][i];
    }
    Ok(beta)
}

#[allow(clippy::needless_range_loop)]
fn cholesky_solve_f32(a: &mut [Vec<f32>], b: &mut [f32]) -> Result<Vec<f32>, NumError> {
    let q = b.len();
    for i in 0..q {
        for j in 0..=i {
            let mut sum = a[i][j];
            for k in 0..j {
                sum -= a[i][k] * a[j][k];
            }
            if i == j {
                if !sum.is_finite() || sum <= 1e-6 {
                    return Err(NumError::Singular {
                        pivot: f64::from(sum),
                    });
                }
                a[i][j] = sum.sqrt();
            } else {
                a[i][j] = sum / a[j][j];
            }
        }
    }
    let mut z = vec![0f32; q];
    for i in 0..q {
        let mut sum = b[i];
        for k in 0..i {
            sum -= a[i][k] * z[k];
        }
        z[i] = sum / a[i][i];
    }
    let mut beta = vec![0f32; q];
    for i in (0..q).rev() {
        let mut sum = z[i];
        for k in (i + 1)..q {
            sum -= a[k][i] * beta[k];
        }
        beta[i] = sum / a[i][i];
    }
    Ok(beta)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_data() -> (Vec<Vec<f64>>, Vec<f64>) {
        let x: Vec<Vec<f64>> = (0..6).map(|i| vec![i as f64]).collect();
        let y: Vec<f64> = x.iter().map(|row| 2.0 * row[0] + 1.0).collect();
        (x, y)
    }

    #[test]
    fn recovers_exact_line() {
        let (x, y) = line_data();
        let fit = ols_f64(&x, &y, 0.0).unwrap();
        assert!((fit.intercept - 1.0).abs() < 1e-12, "{}", fit.intercept);
        assert!((fit.coefficients[0] - 2.0).abs() < 1e-12);
        assert!(fit.residual_sum_squares < 1e-24);
        assert!((predict(&fit, &[10.0]) - 21.0).abs() < 1e-12);
    }

    #[test]
    fn singular_matrix_is_an_error() {
        // 两列完全相同 ⇒ 正规方程奇异（无正则化）。
        let x = vec![
            vec![1.0, 1.0],
            vec![2.0, 2.0],
            vec![3.0, 3.0],
            vec![4.0, 4.0],
        ];
        let y = vec![1.0, 2.0, 3.0, 4.0];
        let err = ols_f64(&x, &y, 0.0).unwrap_err();
        assert!(matches!(err, NumError::Singular { .. }), "{err}");
    }

    #[test]
    fn ridge_handles_collinear_columns() {
        let x = vec![
            vec![1.0, 1.0],
            vec![2.0, 2.0],
            vec![3.0, 3.0],
            vec![4.0, 4.0],
        ];
        let y = vec![1.0, 3.0, 4.0, 7.0];
        let fit = ols_f64(&x, &y, 0.5).unwrap();
        assert!(fit.intercept.is_finite());
        assert!(fit.coefficients.iter().all(|c| c.is_finite()));
    }

    #[test]
    fn input_contracts_are_enforced() {
        let x = vec![vec![1.0], vec![2.0]];
        assert!(matches!(
            ols_f64(&x, &[1.0], 0.0),
            Err(NumError::LabelLength { .. })
        ));
        assert!(matches!(
            ols_f64(&x, &[1.0, f64::NAN], 0.0),
            Err(NumError::NonFinite { .. })
        ));
        assert!(matches!(
            ols_f64(&x, &[1.0, 2.0], -1.0),
            Err(NumError::BadLambda)
        ));
        assert!(matches!(
            ols_f64(&[], &[], 0.0),
            Err(NumError::NotEnoughSamples { .. })
        ));
        let ragged = vec![vec![1.0, 2.0], vec![3.0]];
        assert!(matches!(
            ols_f64(&ragged, &[1.0, 2.0], 0.0),
            Err(NumError::Dimension { .. })
        ));
    }

    #[test]
    fn f32_reference_is_close_on_well_conditioned_data() {
        let (x, y) = line_data();
        let wide: Vec<Vec<f64>> = x.iter().map(|row| vec![row[0], row[0] * row[0]]).collect();
        let fit64 = ols_f64(&wide, &y, 0.0).unwrap();
        let beta32 = ols_f32(&wide, &y, 0.0).unwrap();
        let mut reference = vec![fit64.intercept];
        reference.extend_from_slice(&fit64.coefficients);
        for (a, b) in reference.iter().zip(&beta32) {
            let scale = a.abs().max(1.0);
            assert!(
                ((a - f64::from(*b)).abs() / scale) < 1e-5,
                "f64={a} f32={b}"
            );
        }
    }

    #[test]
    fn results_are_bitwise_deterministic() {
        let (x, y) = line_data();
        let a = ols_f64(&x, &y, 0.0).unwrap();
        let b = ols_f64(&x, &y, 0.0).unwrap();
        assert_eq!(a.intercept.to_bits(), b.intercept.to_bits());
        assert_eq!(a.coefficients[0].to_bits(), b.coefficients[0].to_bits());
    }
}
