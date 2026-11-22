//! 检索上下文：BM25 打分 + 依赖闭包扩展 + token 预算截断。
//!
//! 上下文不进 prompt 全仓库，而是"命中文件 + 它们的依赖"；`bundle_sha256`
//! 对入选文件的路径与内容做内容级指纹，复算时能判断是否同一份上下文。

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use trit_core::fingerprint::sha256_hex;

use crate::repo::Unit;

const K1: f64 = 1.2;
const B: f64 = 0.75;
const MAX_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub score: f64,
    pub tokens: usize,
    /// `hit`（检索命中）或 `dep`（依赖闭包补入）。
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bundle {
    pub query: String,
    pub budget: usize,
    pub hits: Vec<Hit>,
    pub total_tokens: usize,
    pub sha256: String,
}

pub fn bundle(root: &Path, units: &[Unit], query: &str, budget: usize, max_files: usize) -> Bundle {
    let docs = tokens(root, units);
    let n = docs.len().max(1);
    let avg = docs.iter().map(|(_, t)| t.len()).sum::<usize>() as f64 / n as f64;
    let mut df: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, toks) in &docs {
        for t in toks.iter().map(String::as_str).collect::<BTreeSet<_>>() {
            *df.entry(t).or_insert(0) += 1;
        }
    }
    let q = tokenize(query);
    let mut scored: Vec<(usize, f64, usize)> = Vec::new();
    for (i, toks) in &docs {
        let mut tf: BTreeMap<&str, usize> = BTreeMap::new();
        for t in toks {
            *tf.entry(t.as_str()).or_insert(0) += 1;
        }
        let mut score = 0.0;
        for term in &q {
            if let Some(f) = tf.get(term.as_str()) {
                let dfi = df.get(term.as_str()).copied().unwrap_or(0) as f64;
                let idf = ((n as f64 - dfi + 0.5) / (dfi + 0.5) + 1.0).ln();
                let norm = 1.0 - B + B * toks.len() as f64 / avg.max(1.0);
                score += idf * (*f as f64 * (K1 + 1.0)) / (*f as f64 + K1 * norm);
            }
        }
        if score > 0.0 {
            scored.push((*i, score, toks.len()));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    scored.truncate(max_files.max(1));

    let mut hits: Vec<Hit> = Vec::new();
    let mut chosen: BTreeSet<usize> = BTreeSet::new();
    let mut total = 0usize;
    for (i, score, toks) in scored {
        if total + toks > budget && !hits.is_empty() {
            continue;
        }
        total += toks;
        chosen.insert(i);
        hits.push(Hit {
            path: units[i].path.clone(),
            score,
            tokens: toks,
            reason: "hit",
        });
    }
    let pos: BTreeMap<&str, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.path.as_str(), i))
        .collect();
    let mut queue: VecDeque<usize> = chosen.iter().copied().collect();
    while let Some(i) = queue.pop_front() {
        for dep in &units[i].imports_resolved {
            let Some(&j) = pos.get(dep.as_str()) else {
                continue;
            };
            if chosen.contains(&j) {
                continue;
            }
            let toks = docs
                .iter()
                .find(|(k, _)| *k == j)
                .map(|(_, t)| t.len())
                .unwrap_or(0);
            if total + toks > budget {
                continue;
            }
            total += toks;
            chosen.insert(j);
            hits.push(Hit {
                path: units[j].path.clone(),
                score: 0.0,
                tokens: toks,
                reason: "dep",
            });
            queue.push_back(j);
        }
    }
    Bundle {
        query: query.to_string(),
        budget,
        sha256: fingerprint(root, units, &chosen),
        total_tokens: total,
        hits,
    }
}

fn tokens(root: &Path, units: &[Unit]) -> Vec<(usize, Vec<String>)> {
    units
        .iter()
        .enumerate()
        .map(|(i, u)| (i, tokenize(&read_text(root, u))))
        .filter(|(_, t)| !t.is_empty())
        .collect()
}

fn read_text(root: &Path, u: &Unit) -> String {
    if u.bytes as usize > MAX_BYTES {
        return String::new();
    }
    std::fs::read_to_string(root.join(&u.path)).unwrap_or_default()
}

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| s.len() >= 2)
        .map(|s| s.to_lowercase())
        .collect()
}

fn fingerprint(root: &Path, units: &[Unit], chosen: &BTreeSet<usize>) -> String {
    let mut acc = String::new();
    for i in chosen {
        acc.push_str(&units[*i].path);
        acc.push('\0');
        acc.push_str(&read_text(root, &units[*i]));
        acc.push('\0');
    }
    sha256_hex(acc.as_bytes())
}
