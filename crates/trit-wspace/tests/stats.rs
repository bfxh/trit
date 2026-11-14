//! bootstrap 置信区间 / 成对检验 / 分层采样。

use trit_wspace::stats::{bootstrap_spearman_ci, paired_bootstrap_p, spearman, stratified_indices};

fn gold(n: usize) -> Vec<f64> {
    (0..n).map(|index| (index % 17) as f64).collect()
}

#[test]
fn spearman_known_values() {
    // 完全正相关 = 1
    let a: Vec<f64> = (0..20).map(|i| i as f64).collect();
    assert!((spearman(&a, &a) - 1.0).abs() < 1e-12);
    // 完全反相关 = -1
    let reversed: Vec<f64> = a.iter().rev().copied().collect();
    assert!((spearman(&a, &reversed) + 1.0).abs() < 1e-12);
    // 常数序列 → NaN（秩退化）
    let flat = vec![1.0f64; 10];
    assert!(spearman(&flat, &flat).is_nan());
}

#[test]
fn bootstrap_ci_brackets_point_estimate() {
    let g = gold(120);
    // 强信号 + 轻噪声
    let predicted: Vec<f64> = g
        .iter()
        .enumerate()
        .map(|(index, value)| value + ((index % 5) as f64) * 0.1)
        .collect();
    let point = spearman(&predicted, &g);
    let (low, high) = bootstrap_spearman_ci(&predicted, &g, 400, 0.95, 7).expect("ci");
    // percentile bootstrap **不保证**区间包含点估计：估计量有偏时重采样分布会偏斜
    // （Spearman 逼近 1 时就是这种情形）。所以只检查区间合理。
    assert!(low < high, "区间应有宽度");
    assert!(low >= -1.0 && high <= 1.0, "区间应落在相关系数取值范围内");
    assert!(
        (low - point).abs() < 0.1 && (high - point).abs() < 0.1,
        "区间应贴近点估计（区间 {low}..{high} vs 点 {point}）"
    );
}

#[test]
fn bootstrap_ci_is_deterministic() {
    let g = gold(80);
    let predicted: Vec<f64> = g.iter().map(|value| value * 0.5 + 1.0).collect();
    let a = bootstrap_spearman_ci(&predicted, &g, 200, 0.95, 99).expect("a");
    let b = bootstrap_spearman_ci(&predicted, &g, 200, 0.95, 99).expect("b");
    assert_eq!(a, b, "同 seed 必须完全一致");
}

#[test]
fn bootstrap_ci_rejects_tiny_samples() {
    assert!(bootstrap_spearman_ci(&[1.0, 2.0], &[1.0, 2.0], 100, 0.95, 1).is_none());
    assert!(bootstrap_spearman_ci(&[], &[], 100, 0.95, 1).is_none());
}

#[test]
fn paired_bootstrap_detects_clear_difference() {
    let g = gold(200);
    // left：与 gold 无关的噪声；right：接近 gold
    let left: Vec<f64> = (0..200).map(|index| ((index * 37) % 13) as f64).collect();
    let right: Vec<f64> = g.iter().map(|value| value + 0.01).collect();
    let p = paired_bootstrap_p(&left, &right, &g, 300, 5).expect("p");
    assert!(p < 0.05, "明显更好的配置应得到小 p 值，实际 {p}");
}

#[test]
fn paired_bootstrap_same_config_gives_near_half() {
    let g = gold(150);
    let a: Vec<f64> = g.iter().map(|value| value + 0.3).collect();
    let b = a.clone();
    let p = paired_bootstrap_p(&a, &b, &g, 300, 11).expect("p");
    // 完全相同的两侧：差值恒为 0 ⇒ 计入 `<= 0`，p 应为 1.0
    assert!(p > 0.9, "相同配置的 p 应接近 1，实际 {p}");
}

#[test]
fn paired_bootstrap_rejects_bad_shapes() {
    assert!(paired_bootstrap_p(&[1.0], &[1.0, 2.0], &[1.0, 2.0], 10, 1).is_none());
    assert!(paired_bootstrap_p(&[1.0, 2.0], &[1.0, 2.0], &[1.0], 10, 1).is_none());
}

#[test]
fn stratified_keeps_layer_proportions() {
    // 三层，各 40 / 30 / 30
    let mut strata = Vec::new();
    strata.extend(std::iter::repeat(0).take(40));
    strata.extend(std::iter::repeat(1).take(30));
    strata.extend(std::iter::repeat(2).take(30));
    let chosen = stratified_indices(&strata, 0.25, 3);
    let mut counts = [0usize; 3];
    for index in &chosen {
        counts[strata[*index]] += 1;
    }
    assert_eq!(counts, [10, 8, 8], "每层应按 25% 抽样（0.25*30=7.5 → 8）");
    assert_eq!(chosen.len(), 26);
    // 下标不重复且升序
    let mut sorted = chosen.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted, chosen, "下标应唯一且有序");
}

#[test]
fn stratified_handles_tiny_layers() {
    let strata = vec![0, 0, 1];
    let chosen = stratified_indices(&strata, 0.1, 1);
    // 每层至少抽 1 个
    assert_eq!(chosen.len(), 2, "两层各至少 1 个");
    assert!(stratified_indices(&[], 0.5, 1).is_empty());
    assert!(stratified_indices(&[0, 0], 0.0, 1).is_empty());
}
