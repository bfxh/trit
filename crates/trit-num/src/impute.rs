//! 缺失值插补：均值 / 中位数 / 一元线性回归。
//!
//! 输入用 `Option<f64>` 表达缺失；输出**保留原有观测值**，只填缺失位置。
//! 全部缺失或成对样本不足时返回结构化错误，不做“悄悄填 0”。

use crate::NumError;
use crate::ols::{ols_f64, predict};
use crate::stats::median;
use crate::sums::sum_neumaier;

fn observed(values: &[Option<f64>]) -> Result<Vec<f64>, NumError> {
    let mut out = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if let Some(value) = value {
            if !value.is_finite() {
                return Err(NumError::NonFinite {
                    row: index,
                    col: 0,
                    value: *value,
                });
            }
            out.push(*value);
        }
    }
    Ok(out)
}

/// 用观测值的均值填充缺失。
pub fn impute_mean(values: &[Option<f64>]) -> Result<Vec<f64>, NumError> {
    let observed = observed(values)?;
    if observed.is_empty() {
        return Err(NumError::NotEnoughSamples {
            samples: 0,
            needed: 1,
        });
    }
    let mean = sum_neumaier(&observed) / observed.len() as f64;
    Ok(values.iter().map(|value| value.unwrap_or(mean)).collect())
}

/// 用观测值的中位数填充缺失。
pub fn impute_median(values: &[Option<f64>]) -> Result<Vec<f64>, NumError> {
    let observed = observed(values)?;
    if observed.is_empty() {
        return Err(NumError::NotEnoughSamples {
            samples: 0,
            needed: 1,
        });
    }
    let fill = median(&observed)?;
    Ok(values.iter().map(|value| value.unwrap_or(fill)).collect())
}

/// 一元线性回归插补：用完整成对样本拟合 `y = a + b·x`，再补 `y` 的缺失。
///
/// `x` 必须完整（缺自变量不属于本函数的口径）；`x` 全相同时会因奇异返回错误。
pub fn impute_linear(x: &[f64], y: &[Option<f64>]) -> Result<Vec<f64>, NumError> {
    if x.len() != y.len() {
        return Err(NumError::LabelLength {
            x: x.len(),
            y: y.len(),
        });
    }
    for (index, value) in x.iter().enumerate() {
        if !value.is_finite() {
            return Err(NumError::NonFinite {
                row: index,
                col: 0,
                value: *value,
            });
        }
    }
    let mut design = Vec::new();
    let mut labels = Vec::new();
    for (value, label) in x.iter().zip(y) {
        if let Some(label) = label {
            if !label.is_finite() {
                return Err(NumError::NonFinite {
                    row: labels.len(),
                    col: 0,
                    value: *label,
                });
            }
            design.push(vec![*value]);
            labels.push(*label);
        }
    }
    if labels.len() < 2 {
        return Err(NumError::NotEnoughSamples {
            samples: labels.len(),
            needed: 2,
        });
    }
    let fit = ols_f64(&design, &labels, 0.0)?;
    Ok(x.iter()
        .zip(y)
        .map(|(value, label)| label.unwrap_or_else(|| predict(&fit, &[*value])))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_and_median_impute_fill_only_missing() {
        let values = [Some(1.0), None, Some(3.0)];
        assert_eq!(impute_mean(&values).unwrap(), vec![1.0, 2.0, 3.0]);
        let values = [Some(10.0), None, Some(2.0), Some(6.0)];
        assert_eq!(impute_median(&values).unwrap(), vec![10.0, 6.0, 2.0, 6.0]);
    }

    #[test]
    fn all_missing_is_an_error() {
        let values = [None, None];
        assert!(matches!(
            impute_mean(&values),
            Err(NumError::NotEnoughSamples { .. })
        ));
        assert!(matches!(
            impute_median(&values),
            Err(NumError::NotEnoughSamples { .. })
        ));
    }

    #[test]
    fn linear_impute_recovers_exact_line() {
        // y = 3x − 2，中间两格缺失。
        let x: Vec<f64> = (0..6).map(|i| i as f64).collect();
        let y = [Some(-2.0), None, Some(4.0), None, Some(10.0), Some(13.0)];
        let filled = impute_linear(&x, &y).unwrap();
        for (i, value) in filled.iter().enumerate() {
            let expected = 3.0 * i as f64 - 2.0;
            assert!(
                (value - expected).abs() < 1e-12,
                "i={i} {value} vs {expected}"
            );
        }
    }

    #[test]
    fn linear_impute_rejects_degenerate_inputs() {
        // x 全相同 ⇒ 斜率不可辨识。
        let x = [1.0, 1.0, 1.0];
        let y = [Some(1.0), Some(2.0), None];
        assert!(matches!(
            impute_linear(&x, &y),
            Err(NumError::Singular { .. })
        ));
        // 成对样本不足。
        let x = [1.0, 2.0, 3.0];
        let y = [Some(1.0), None, None];
        assert!(matches!(
            impute_linear(&x, &y),
            Err(NumError::NotEnoughSamples { .. })
        ));
        // 长度不一致。
        assert!(matches!(
            impute_linear(&[1.0], &[Some(1.0), None]),
            Err(NumError::LabelLength { .. })
        ));
    }
}
