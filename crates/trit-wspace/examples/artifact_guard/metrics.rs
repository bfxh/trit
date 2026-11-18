//! `artifact_guard` 的共享部件：随机源与二值评测（合法 vs 攻击）。

/// xorshift64*。
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
}

use serde_json::{Value, json};

/// 二值口径（合法=0 vs 攻击=1）：acc / 漏检率（攻击被判合法的比例）。
pub(super) fn evaluate_binary(preds: &[(usize, usize)]) -> Value {
    let mut tp = 0usize; // 攻击判为攻击
    let mut fn_ = 0usize; // 攻击判为合法（漏检）
    let mut tn = 0usize; // 合法判为合法
    let mut fp = 0usize; // 合法判为攻击（误报）
    for &(pred, label) in preds {
        let pred_attack = pred != 0;
        let is_attack = label != 0;
        match (pred_attack, is_attack) {
            (true, true) => tp += 1,
            (false, true) => fn_ += 1,
            (false, false) => tn += 1,
            (true, false) => fp += 1,
        }
    }
    let acc = (tp + tn) as f32 / (tp + tn + fp + fn_).max(1) as f32;
    let recall = tp as f32 / (tp + fn_).max(1) as f32;
    let precision = tp as f32 / (tp + fp).max(1) as f32;
    json!({
        "acc": acc,
        "attack_recall": recall,
        "attack_precision": precision,
        "miss_rate": fn_ as f32 / (tp + fn_).max(1) as f32,
        "false_positive_rate": fp as f32 / (fp + tn).max(1) as f32,
    })
}
