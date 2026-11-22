//! 与外部 judge 的一致性：多类 Cohen's κ（可复算）。

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Kappa {
    pub n: usize,
    pub observed: f64,
    pub expected: f64,
    pub kappa: f64,
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
    Ok(Kappa {
        n: a.len(),
        observed,
        expected,
        kappa,
        counts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn perfect_agreement_is_one() {
        let k = cohen_kappa(
            &v(&["accept", "reject", "defer"]),
            &v(&["accept", "reject", "defer"]),
        )
        .unwrap();
        assert_eq!(k.n, 3);
        assert!((k.kappa - 1.0).abs() < 1e-12);
    }

    #[test]
    fn known_zero_kappa_example() {
        let k = cohen_kappa(
            &v(&["yes", "no", "yes", "no"]),
            &v(&["yes", "no", "no", "yes"]),
        )
        .unwrap();
        assert!((k.observed - 0.5).abs() < 1e-12);
        assert!((k.expected - 0.5).abs() < 1e-12);
        assert!(k.kappa.abs() < 1e-12);
    }

    #[test]
    fn length_mismatch_is_an_error() {
        assert!(cohen_kappa(&v(&["a"]), &v(&[])).is_err());
    }
}
