//! 采样与显著性工具：bootstrap 置信区间、成对检验、分层采样。
//!
//! ## 为什么需要这个模块
//!
//! 这一路的实验一直以**单点 Spearman** 下结论，由此踩了两个坑：
//!
//! 1. **子集有偏**：STS test 由 6 个子数据集拼成，取「前 200 对」得到 Spearman 0.8614，
//!    全量 1379 对只有 0.7311——差 13 个点，纯粹是抽样造成的。
//! 2. **无法判断差异是否真实**：三值分组只掉 0.0004 点，可这个数在 300/1379 对上的
//!    抽样标准误有多大？没有区间就说不清"几乎无损"是不是巧合。
//!
//! 对应两个标准工具（经典方法，见文末引用）：
//!
//! - [`bootstrap_spearman_ci`]：percentile bootstrap 置信区间，给单点读数配一个区间；
//! - [`paired_bootstrap_p`]：**成对** bootstrap（同一批样本上重采样），检验两个配置的差异；
//! - [`stratified_indices`]：按 strata 分层抽样，让小样本子集保持各层比例。
//!
//! 全部确定性（LCG），同一 `seed` 必得同一结果。
//!
//! **注意**：percentile bootstrap 的区间**不保证包含点估计**——估计量有偏时重采样分布会偏斜
//! （Spearman 逼近 1 时正是如此）。需要"必然包含点估计"时应用 basic bootstrap
//! （`2θ̂ − q_high, 2θ̂ − q_low`），本模块只提供更常用的 percentile 版本。
//!
//! ## 引用
//!
//! - Efron & Tibshirani, *An Introduction to the Bootstrap* (1993) —— percentile bootstrap。
//! - Dietterich, *Approximate Statistical Tests for Comparing Supervised Classification
//!   Learning Algorithms*, Neural Computation 10(7), 1998 —— 为什么必须**成对**比较。
//! - Koehn, *Statistical Significance Tests for Machine Translation Evaluation*, EMNLP 2004
//!   —— 成对 bootstrap 重采样的具体做法（paired bootstrap resampling）。

/// 确定性 LCG（与仓库其余部分一致）。
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) % bound.max(1) as u64) as usize
    }
}

/// 平均秩（并列取均值）。
fn ranks(values: &[f64]) -> Vec<f64> {
    let mut order: Vec<usize> = (0..values.len()).collect();
    order.sort_by(|left, right| {
        values[*left]
            .partial_cmp(&values[*right])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut result = vec![0.0f64; values.len()];
    let mut index = 0usize;
    while index < order.len() {
        let mut end = index + 1;
        while end < order.len() && (values[order[end]] - values[order[index]]).abs() < 1e-12 {
            end += 1;
        }
        let average = (index + end - 1) as f64 / 2.0 + 1.0;
        for slot in index..end {
            result[order[slot]] = average;
        }
        index = end;
    }
    result
}

/// Spearman 相关（秩上的 Pearson）。
#[must_use]
pub fn spearman(left: &[f64], right: &[f64]) -> f64 {
    if left.len() != right.len() || left.len() < 2 {
        return f64::NAN;
    }
    let lr = ranks(left);
    let rr = ranks(right);
    let n = lr.len() as f64;
    let mean_l: f64 = lr.iter().sum::<f64>() / n;
    let mean_r: f64 = rr.iter().sum::<f64>() / n;
    let mut cov = 0.0;
    let mut var_l = 0.0;
    let mut var_r = 0.0;
    for (a, b) in lr.iter().zip(&rr) {
        let da = a - mean_l;
        let db = b - mean_r;
        cov += da * db;
        var_l += da * da;
        var_r += db * db;
    }
    if var_l == 0.0 || var_r == 0.0 {
        f64::NAN
    } else {
        cov / (var_l.sqrt() * var_r.sqrt())
    }
}

/// percentile bootstrap 置信区间（对样本对重采样）。
///
/// 返回 `(下界, 上界)`；样本不足或秩退化时返回 `None`。
#[must_use]
pub fn bootstrap_spearman_ci(
    predicted: &[f64],
    gold: &[f64],
    resamples: usize,
    level: f64,
    seed: u64,
) -> Option<(f64, f64)> {
    if predicted.len() != gold.len() || predicted.len() < 4 || resamples == 0 {
        return None;
    }
    let n = predicted.len();
    let mut rng = Lcg::new(seed);
    let mut values = Vec::with_capacity(resamples);
    for _ in 0..resamples {
        let mut xs = Vec::with_capacity(n);
        let mut ys = Vec::with_capacity(n);
        for _ in 0..n {
            let index = rng.next_usize(n);
            xs.push(predicted[index]);
            ys.push(gold[index]);
        }
        let value = spearman(&xs, &ys);
        if value.is_finite() {
            values.push(value);
        }
    }
    if values.len() < 2 {
        return None;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let level = level.clamp(0.0, 1.0);
    let tail = (1.0 - level) / 2.0;
    let low = ((tail * values.len() as f64) as usize).min(values.len() - 1);
    let high = (((1.0 - tail) * values.len() as f64) as usize).min(values.len() - 1);
    Some((values[low], values[high]))
}

/// 成对 bootstrap 检验：`P(spearman(right) − spearman(left) <= 0)`。
///
/// 每次重采样都**用同一组下标**同时评估两侧（这是 Dietterich 强调的"成对"），
/// 于是样本本身的难度被消掉，只剩下配置差异。返回单侧 p 值：越小说明 right 越好越可信。
#[must_use]
pub fn paired_bootstrap_p(
    left: &[f64],
    right: &[f64],
    gold: &[f64],
    resamples: usize,
    seed: u64,
) -> Option<f64> {
    if left.len() != gold.len() || right.len() != gold.len() || left.len() < 4 || resamples == 0 {
        return None;
    }
    let n = left.len();
    let mut rng = Lcg::new(seed);
    let mut wins = 0usize;
    let mut counted = 0usize;
    for _ in 0..resamples {
        let mut indices = Vec::with_capacity(n);
        for _ in 0..n {
            indices.push(rng.next_usize(n));
        }
        let pick = |source: &[f64]| {
            indices
                .iter()
                .map(|index| source[*index])
                .collect::<Vec<f64>>()
        };
        let gold_sample = pick(gold);
        let a = spearman(&pick(left), &gold_sample);
        let b = spearman(&pick(right), &gold_sample);
        if !a.is_finite() || !b.is_finite() {
            continue;
        }
        counted += 1;
        if b - a <= 0.0 {
            wins += 1;
        }
    }
    if counted == 0 {
        None
    } else {
        Some(wins as f64 / counted as f64)
    }
}

/// 按 strata 分层抽样，返回被选中的下标（保持各层比例）。
///
/// `strata[i]` 是第 `i` 个样本所属的层号；每层按相同比例抽 `ratio`（不足 1 个时补 1 个）。
#[must_use]
pub fn stratified_indices(strata: &[usize], ratio: f64, seed: u64) -> Vec<usize> {
    if strata.is_empty() || ratio <= 0.0 {
        return Vec::new();
    }
    let ratio = ratio.min(1.0);
    let mut buckets: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for (index, stratum) in strata.iter().enumerate() {
        buckets.entry(*stratum).or_default().push(index);
    }
    let mut rng = Lcg::new(seed);
    let mut chosen = Vec::new();
    for indices in buckets.values() {
        let want = ((indices.len() as f64 * ratio).round() as usize).clamp(1, indices.len());
        // 层内确定性洗牌后取前 want 个
        let mut pool = indices.clone();
        for position in (1..pool.len()).rev() {
            let swap = rng.next_usize(position + 1);
            pool.swap(position, swap);
        }
        chosen.extend_from_slice(&pool[..want]);
    }
    chosen.sort_unstable();
    chosen
}
