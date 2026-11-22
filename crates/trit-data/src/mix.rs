//! 配比：按标签分层的配额（简化 DoReMi/RegMix 的“先定权重再配量”）。

use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Quota {
    pub label: String,
    pub weight: f64,
    pub available: usize,
    pub take: usize,
}

pub fn allocate(
    counts: &BTreeMap<String, usize>,
    total: usize,
    weights: &BTreeMap<String, f64>,
) -> Vec<Quota> {
    if counts.is_empty() || total == 0 {
        return Vec::new();
    }
    let w = |k: &String| weights.get(k).copied().unwrap_or(1.0).max(0.0);
    let sum: f64 = counts.keys().map(w).sum();
    if sum <= 0.0 {
        return counts
            .iter()
            .map(|(k, &a)| Quota {
                label: k.clone(),
                weight: 1.0,
                available: a,
                take: 0,
            })
            .collect();
    }
    let mut quotas: Vec<Quota> = counts
        .iter()
        .map(|(k, &a)| {
            let ideal = total as f64 * w(k) / sum;
            Quota {
                label: k.clone(),
                weight: w(k),
                available: a,
                take: (ideal.floor() as usize).min(a),
            }
        })
        .collect();
    let taken: usize = quotas.iter().map(|q| q.take).sum();
    let mut left = total.saturating_sub(taken);
    let mut idx: Vec<usize> = (0..quotas.len()).collect();
    idx.sort_by(|&i, &j| {
        let ri = total as f64 * quotas[i].weight / sum - quotas[i].take as f64;
        let rj = total as f64 * quotas[j].weight / sum - quotas[j].take as f64;
        rj.partial_cmp(&ri).unwrap_or(std::cmp::Ordering::Equal)
    });
    while left > 0 {
        let mut advanced = false;
        for &i in &idx {
            if left == 0 {
                break;
            }
            if quotas[i].take < quotas[i].available {
                quotas[i].take += 1;
                left -= 1;
                advanced = true;
            }
        }
        if !advanced {
            break;
        }
    }
    quotas
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(pairs: &[(&str, usize)]) -> BTreeMap<String, usize> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn weights(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn equal_weights_split_evenly() {
        let q = allocate(&counts(&[("a", 100), ("b", 100)]), 100, &weights(&[]));
        assert_eq!(q.iter().map(|x| x.take).sum::<usize>(), 100);
        assert_eq!(q[0].take, 50);
        assert_eq!(q[1].take, 50);
    }

    #[test]
    fn weights_are_respected() {
        let q = allocate(
            &counts(&[("code", 100), ("writing", 50)]),
            80,
            &weights(&[("code", 0.7), ("writing", 0.3)]),
        );
        let code = q.iter().find(|x| x.label == "code").unwrap();
        let writing = q.iter().find(|x| x.label == "writing").unwrap();
        assert_eq!((code.take, writing.take), (56, 24));
    }

    #[test]
    fn scarce_labels_are_capped_and_remainder_filled() {
        let q = allocate(
            &counts(&[("a", 10), ("b", 100)]),
            100,
            &weights(&[("a", 0.5), ("b", 0.5)]),
        );
        let a = q.iter().find(|x| x.label == "a").unwrap();
        let b = q.iter().find(|x| x.label == "b").unwrap();
        assert_eq!(a.take, 10);
        assert_eq!(b.take, 90);
    }
}
