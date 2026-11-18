//! `spam_redteam` 的共享部件：随机源、哈希特征、划分与评测指标。

use std::fs;

/// xorshift64*（同 seed 同划分/同攻击）。
pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub(super) fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub(super) fn next_f32(&mut self) -> f32 {
        (self.next_u64() % 2001) as f32 / 1000.0 - 1.0
    }
    pub(super) fn next_index(&mut self, bound: usize) -> usize {
        usize::try_from(self.next_u64() % bound as u64).expect("bound > 0")
    }
}

pub(super) fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 加载 `label<TAB>text`；ham=0、spam=1。
pub(super) fn load_dataset(path: &str) -> Vec<(usize, String)> {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("读 {path}: {error}"));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (label, body) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("行不是 label<TAB>text：{line:.40}"));
            let label = match label.trim() {
                "ham" => 0,
                "spam" => 1,
                other => panic!("未知标签 {other}"),
            };
            (label, body.to_string())
        })
        .collect()
}

/// 字符 3-gram/4-gram 哈希特征：出现=+1（三值输入的存在性编码）。
pub(super) const FEAT_DIM: usize = 4096;

pub(super) fn featurize(text: &str) -> Vec<f32> {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let mut feats = vec![0.0f32; FEAT_DIM];
    for width in [3usize, 4] {
        if lower.len() < width {
            continue;
        }
        for start in 0..=(lower.len() - width) {
            let mut bytes = Vec::with_capacity(width * 2);
            for ch in &lower[start..start + width] {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            let slot = (fnv(&bytes) % FEAT_DIM as u64) as usize;
            feats[slot] = 1.0;
        }
    }
    feats
}

/// 分层划分：每个类别内 shuffle 后按 70/15/15 切。
pub(super) fn stratified_split(
    labels: &[usize],
    rng: &mut Rng,
) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut by_class: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for (index, &label) in labels.iter().enumerate() {
        by_class[label].push(index);
    }
    let mut train = Vec::new();
    let mut val = Vec::new();
    let mut test = Vec::new();
    for group in &mut by_class {
        let n = group.len();
        for i in (1..n).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            group.swap(i, j);
        }
        let train_end = n * 7 / 10;
        let val_end = n * 85 / 100;
        train.extend(&group[..train_end]);
        val.extend(&group[train_end..val_end]);
        test.extend(&group[val_end..]);
    }
    (train, val, test)
}
