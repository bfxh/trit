//! 与外部 judge 的一致性：多类 Cohen's κ（可复算）。

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Kappa {
    pub n: usize,
    pub observed: f64,
    pub expected: f64,
    pub kappa: f64,
    pub se: f64,
    pub ci_low: f64,
    pub ci_high: f64,
    pub counts: BTreeMap<(String, String), usize>,
}

pub fn cohen_kappa(a: &[String], b: &[String]) -> Result<Kappa, String> {
    if a.len() != b.len() {
        return Err(format!("长度不一致：{} vs {}", a.len(), b.len()));
    }
    if a.is_empty() {
        return Err("样本为空".into());
    }
    let mut counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut row: BTreeMap<String, usize> = BTreeMap::new();
    let mut col: BTreeMap<String, usize> = BTreeMap::new();
    let mut diag = 0usize;
    for (x, y) in a.iter().zip(b) {
        *counts.entry((x.clone(), y.clone())).or_insert(0) += 1;
        *row.entry(x.clone()).or_insert(0) += 1;
        *col.entry(y.clone()).or_insert(0) += 1;
        if x == y {
            diag += 1;
        }
    }
    let n = a.len() as f64;
    let observed = diag as f64 / n;
    let expected: f64 = row
        .iter()
        .map(|(k, v)| {
            let c = col.get(k).copied().unwrap_or(0) as f64;
            (*v as f64 / n) * (c / n)
        })
        .sum();
    let kappa = if (1.0 - expected).abs() < 1e-12 {
        1.0
    } else {
        (observed - expected) / (1.0 - expected)
    };
    let se = if (1.0 - expected).abs() < 1e-12 {
        0.0
    } else {
        (observed * (1.0 - observed) / (a.len() as f64 * (1.0 - expected).powi(2))).sqrt()
    };
    let ci_low = (kappa - 1.96 * se).clamp(-1.0, 1.0);
    let ci_high = (kappa + 1.96 * se).clamp(-1.0, 1.0);
    Ok(Kappa {
        n: a.len(),
        observed,
        expected,
        kappa,
        se,
        ci_low,
        ci_high,
        counts,
    })
}

#[cfg(test)]
#[path = "compare_tests.rs"]
mod tests;
