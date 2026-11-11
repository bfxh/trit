//! 补偿求和与 Welford 流式统计。

/// Neumaier 补偿求和：比朴素累加稳定，且对同一输入 + 同一顺序**逐位确定**。
#[must_use]
pub fn sum_neumaier(values: &[f64]) -> f64 {
    let mut sum = 0.0f64;
    let mut compensation = 0.0f64;
    for &value in values {
        let total = sum + value;
        if sum.abs() >= value.abs() {
            compensation += (sum - total) + value;
        } else {
            compensation += (value - total) + sum;
        }
        sum = total;
    }
    sum + compensation
}

/// Welford 流式均值/方差（可合并；只用 n / mean / m2 三个量）。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Welford {
    count: u64,
    mean: f64,
    m2: f64,
}

impl Welford {
    /// 空统计量。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 加入一个样本。
    pub fn push(&mut self, value: f64) {
        self.count += 1;
        let delta = value - self.mean;
        self.mean += delta / self.count as f64;
        self.m2 += delta * (value - self.mean);
    }

    /// 样本数。
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// 均值；空集返回 `None`。
    #[must_use]
    pub fn mean(&self) -> Option<f64> {
        (self.count > 0).then_some(self.mean)
    }

    /// 样本方差（除以 `n-1`）；`n < 2` 返回 `None`。
    #[must_use]
    pub fn variance(&self) -> Option<f64> {
        (self.count >= 2).then(|| self.m2 / (self.count - 1) as f64)
    }

    /// 合并另一段统计量（两段必须是同一样本空间的两部分）。
    pub fn merge(&mut self, other: &Self) {
        if other.count == 0 {
            return;
        }
        if self.count == 0 {
            *self = *other;
            return;
        }
        let n1 = self.count as f64;
        let n2 = other.count as f64;
        let n = n1 + n2;
        let delta = other.mean - self.mean;
        self.mean += delta * n2 / n;
        self.m2 += other.m2 + delta * delta * n1 * n2 / n;
        self.count += other.count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neumaier_sum_survives_cancellation() {
        // 朴素累加会把这个和算成 0；补偿求和必须得到 1。
        let values = [1e16, 1.0, -1e16];
        assert_eq!(sum_neumaier(&values), 1.0);
        let naive: f64 = values.iter().sum();
        assert_eq!(naive, 0.0, "朴素累加确实是 0（对照）");
    }

    #[test]
    fn welford_matches_naive_moments() {
        let values = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let mut stats = Welford::new();
        for value in values {
            stats.push(value);
        }
        assert_eq!(stats.count(), 8);
        let mean = sum_neumaier(&values) / values.len() as f64;
        assert!((stats.mean().unwrap() - mean).abs() < 1e-12);
        let naive_var: f64 = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / 7.0;
        assert!((stats.variance().unwrap() - naive_var).abs() < 1e-12);
    }

    #[test]
    fn welford_merge_matches_one_pass() {
        let left = [3.0, 1.0, 4.0, 1.0];
        let right = [5.0, 9.0, 2.0, 6.0, 5.0];
        let mut one = Welford::new();
        for value in left.iter().chain(right.iter()) {
            one.push(*value);
        }
        let mut a = Welford::new();
        for value in left {
            a.push(value);
        }
        let mut b = Welford::new();
        for value in right {
            b.push(value);
        }
        a.merge(&b);
        assert_eq!(a.count(), one.count());
        assert!((a.mean().unwrap() - one.mean().unwrap()).abs() < 1e-12);
        assert!((a.variance().unwrap() - one.variance().unwrap()).abs() < 1e-12);
    }

    #[test]
    fn empty_moments_are_none() {
        let stats = Welford::new();
        assert!(stats.mean().is_none());
        assert!(stats.variance().is_none());
        let mut one = Welford::new();
        one.push(1.0);
        assert_eq!(one.mean(), Some(1.0));
        assert!(one.variance().is_none());
    }
}
