//! 中位数与中位绝对偏差（MAD）。

use crate::NumError;
use crate::sums::sum_neumaier;

/// 中位数（偶数个取中间两数的平均；用补偿求和求平均）。
pub fn median(values: &[f64]) -> Result<f64, NumError> {
    if values.is_empty() {
        return Err(NumError::Empty);
    }
    for (i, value) in values.iter().enumerate() {
        if !value.is_finite() {
            return Err(NumError::NonFinite {
                row: i,
                col: 0,
                value: *value,
            });
        }
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    if n % 2 == 1 {
        Ok(sorted[n / 2])
    } else {
        Ok(sum_neumaier(&[sorted[n / 2 - 1], sorted[n / 2]]) / 2.0)
    }
}

/// 中位绝对偏差：`median(|x − median(x)|)`（未乘 1.4826；需要正态一致性系数由调用方决定）。
pub fn mad(values: &[f64]) -> Result<f64, NumError> {
    let center = median(values)?;
    let deviations: Vec<f64> = values.iter().map(|value| (value - center).abs()).collect();
    median(&deviations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_matches_sorted_reference() {
        assert_eq!(median(&[3.0, 1.0, 2.0]).unwrap(), 2.0);
        assert_eq!(median(&[4.0, 1.0, 3.0, 2.0]).unwrap(), 2.5);
        assert_eq!(median(&[5.0]).unwrap(), 5.0);
        assert!(matches!(median(&[]), Err(NumError::Empty)));
        assert!(matches!(
            median(&[1.0, f64::INFINITY]),
            Err(NumError::NonFinite { .. })
        ));
    }

    #[test]
    fn mad_is_median_absolute_deviation() {
        // 中位数 3；偏差 [2,1,0,1,2] ⇒ MAD = 1。
        assert_eq!(mad(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap(), 1.0);
        // 偶数样本：中位数 2.5；偏差 [1.5,.5,.5,1.5] ⇒ 中位数 1.0。
        assert_eq!(mad(&[1.0, 2.0, 3.0, 4.0]).unwrap(), 1.0);
        assert!(mad(&[]).is_err());
    }
}
