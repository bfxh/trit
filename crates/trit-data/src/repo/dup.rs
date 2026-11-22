//! 文件级去重：精确（内容 SHA-256）与近似（MinHash + LSH）。
//!
//! 只对「同内容的第一份」做近似去重，避免重复文件把相似度放大；测试文件、
//! 过小（<200B）与过大（>1MB）的文件跳过并在报告里计数，不静默丢弃。

use std::collections::BTreeMap;
use std::path::Path;

use trit_core::fingerprint::sha256_hex;

use crate::repo::Unit;

pub const SIGNATURES: usize = 64;
const BANDS: usize = 16;
const ROWS: usize = 4;
const SHINGLE: usize = 5;
const MAX_SHINGLES: usize = 4000;
const MIN_BYTES: u64 = 200;
const MAX_BYTES: u64 = 1 << 20;
const MAX_BUCKET: usize = 64;
pub const THRESHOLD: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactGroup {
    pub hash: String,
    pub paths: Vec<String>,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NearGroup {
    pub paths: Vec<String>,
    pub similarity: f64,
}

#[derive(Debug, Clone, Default)]
pub struct Dup {
    pub files: usize,
    pub skipped: usize,
    pub exact: Vec<ExactGroup>,
    pub near: Vec<NearGroup>,
}

pub fn scan(root: &Path, units: &[Unit]) -> Dup {
    let mut dup = Dup::default();
    let mut by_hash: BTreeMap<String, (Vec<String>, u64)> = BTreeMap::new();
    let mut uniques: Vec<(String, Vec<u64>)> = Vec::new();
    for u in units {
        let size = std::fs::metadata(root.join(&u.path))
            .map(|m| m.len())
            .unwrap_or(0);
        if !(MIN_BYTES..=MAX_BYTES).contains(&size) || u.is_test {
            dup.skipped += 1;
            continue;
        }
        let text = match std::fs::read_to_string(root.join(&u.path)) {
            Ok(t) => t,
            Err(_) => {
                dup.skipped += 1;
                continue;
            }
        };
        let hash = sha256_hex(text.as_bytes());
        let entry = by_hash.entry(hash).or_insert_with(|| (Vec::new(), size));
        entry.0.push(u.path.clone());
        if entry.0.len() == 1 {
            uniques.push((u.path.clone(), shingles(&text)));
        }
        dup.files += 1;
    }
    for (hash, (paths, bytes)) in by_hash {
        if paths.len() > 1 {
            dup.exact.push(ExactGroup { hash, paths, bytes });
        }
    }
    dup.near = near_groups(&uniques);
    dup
}

/// 词级 5-shingle 的哈希集合（排序去重）；上限 `MAX_SHINGLES` 条。
fn shingles(text: &str) -> Vec<u64> {
    let toks: Vec<u64> = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .map(|s| fnv1a(s.as_bytes()))
        .collect();
    if toks.len() < SHINGLE {
        return Vec::new();
    }
    let mut out = Vec::new();
    for w in toks.windows(SHINGLE) {
        let mut h = 0u64;
        for t in w {
            h = h.wrapping_mul(0x100000001B3) ^ t;
        }
        out.push(h);
        if out.len() >= MAX_SHINGLES {
            break;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

fn signature(set: &[u64]) -> [u64; SIGNATURES] {
    let mut sig = [u64::MAX; SIGNATURES];
    for (i, slot) in sig.iter_mut().enumerate() {
        let seed = mix(i as u64 ^ 0x5EED);
        for h in set {
            let v = mix(h ^ seed);
            if v < *slot {
                *slot = v;
            }
        }
    }
    sig
}

fn band_hash(rows: &[u64]) -> u64 {
    let mut h = 0u64;
    for r in rows {
        h = mix(h ^ *r);
    }
    h
}

fn jaccard(a: &[u64], b: &[u64]) -> f64 {
    let (mut i, mut j, mut inter) = (0usize, 0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            inter += 1;
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    let union = a.len() + b.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

fn near_groups(uniques: &[(String, Vec<u64>)]) -> Vec<NearGroup> {
    let sigs: Vec<[u64; SIGNATURES]> = uniques.iter().map(|(_, s)| signature(s)).collect();
    let mut buckets: BTreeMap<(usize, u64), Vec<usize>> = BTreeMap::new();
    for (i, sig) in sigs.iter().enumerate() {
        for b in 0..BANDS {
            let key = band_hash(&sig[b * ROWS..(b + 1) * ROWS]);
            buckets.entry((b, key)).or_default().push(i);
        }
    }
    let mut pairs: BTreeMap<(usize, usize), f64> = BTreeMap::new();
    for group in buckets.values() {
        if group.len() < 2 || group.len() > MAX_BUCKET {
            continue;
        }
        for a in 0..group.len() {
            for b in (a + 1)..group.len() {
                let (x, y) = (group[a], group[b]);
                let sim = jaccard(&uniques[x].1, &uniques[y].1);
                if sim >= THRESHOLD {
                    pairs.insert((x.min(y), x.max(y)), sim);
                }
            }
        }
    }
    let mut uf = UnionFind::new(uniques.len());
    for (a, b) in pairs.keys() {
        uf.union(*a, *b);
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..uniques.len() {
        let r = uf.find(i);
        groups.entry(r).or_default().push(i);
    }
    let mut out = Vec::new();
    for members in groups.values() {
        if members.len() < 2 {
            continue;
        }
        let mut sim = f64::MAX;
        for (i, a) in members.iter().enumerate() {
            for b in &members[i + 1..] {
                if let Some(p) = pairs.get(&(*a.min(b), *a.max(b))) {
                    sim = sim.min(*p);
                }
            }
        }
        let mut paths: Vec<String> = members.iter().map(|m| uniques[*m].0.clone()).collect();
        paths.sort();
        out.push(NearGroup {
            similarity: if sim == f64::MAX { 0.0 } else { sim },
            paths,
        });
    }
    out.sort_by(|a, b| a.paths.cmp(&b.paths));
    out
}

struct UnionFind {
    parent: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            let r = self.find(self.parent[x]);
            self.parent[x] = r;
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent[ra] = rb;
        }
    }
}
