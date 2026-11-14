//! 分层划分：每类按 67% 分训练/测试（同一 seed 结果固定）。

use super::{CLASSES, Sample};

pub(super) fn split(all: &[Sample], seed: u64) -> (Vec<Sample>, Vec<Sample>) {
    let mut state = seed | 1;
    let (mut train, mut test) = (Vec::new(), Vec::new());
    for label in 0..CLASSES {
        let mut indices: Vec<usize> = (0..all.len()).filter(|&i| all[i].label == label).collect();
        for slot in (1..indices.len()).rev() {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            indices.swap(slot, (state % (slot as u64 + 1)) as usize);
        }
        let cut = indices.len() * 2 / 3;
        train.extend(indices[..cut].iter().map(|&i| all[i].clone()));
        test.extend(indices[cut..].iter().map(|&i| all[i].clone()));
    }
    (train, test)
}
