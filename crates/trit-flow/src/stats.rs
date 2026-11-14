//! 读数：延迟分位数与吞吐。
//!
//! 口径写死在这里，报告里不许换：
//! - 延迟是**每个请求从进入执行器到完成**的墙钟毫秒数；
//! - `p50/p90/p99` 用线性插值分位（与 NumPy 默认一致），不是最近秩；
//! - 吞吐是 `请求数 / 墙钟秒`，另有 `样本·步/秒` 这一项用来跨步数配置比较。

/// 一组延迟读数（毫秒）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Latency {
    /// 样本数。
    pub n: usize,
    /// 最小。
    pub min: f64,
    /// 中位数。
    pub p50: f64,
    /// 90 分位。
    pub p90: f64,
    /// 99 分位。
    pub p99: f64,
    /// 最大。
    pub max: f64,
    /// 均值。
    pub mean: f64,
}

/// 线性插值分位（`q ∈ [0,1]`），输入必须已升序。
///
/// # Panics
/// `samples` 为空或 `q` 不在 `[0,1]`。
#[must_use]
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    assert!(!sorted.is_empty(), "分位需要非空样本");
    assert!((0.0..=1.0).contains(&q), "分位参数必须在 [0,1]");
    if sorted.len() == 1 {
        return sorted[0];
    }
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        sorted[lo]
    } else {
        let w = pos - lo as f64;
        sorted[lo] * (1.0 - w) + sorted[hi] * w
    }
}

/// 把一组延迟样本汇总成读数（内部排序，不改调用方切片）。
///
/// # Panics
/// 样本为空。
#[must_use]
pub fn summarize(samples: &[f64]) -> Latency {
    assert!(!samples.is_empty(), "读数需要至少一个样本");
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).expect("延迟读数不应含 NaN"));
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    Latency {
        n,
        min: v[0],
        p50: percentile(&v, 0.50),
        p90: percentile(&v, 0.90),
        p99: percentile(&v, 0.99),
        max: v[n - 1],
        mean,
    }
}
