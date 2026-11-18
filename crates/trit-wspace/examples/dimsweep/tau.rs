//! dimsweep 的保真阈值曲线：对每个 τ 报「使 agree_full ≥ τ 的最小 k」。
//!
//! 从 `evaluate.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use serde_json::{Value, json};

/// 保真阈值候选：`tau_curve` 对每个 τ 报「使 agree_full ≥ τ 的最小 k」。
pub const TAUS: [f32; 8] = [0.99, 0.95, 0.90, 0.80, 0.75, 0.70, 0.65, 0.60];

/// 对每个 τ 报「使 `agree_full ≥ τ` 的最小 k」；没有 k 达到就报 null。
pub fn tau_curve(pairs: &[(usize, f32)]) -> Vec<Value> {
    TAUS.iter()
        .map(|&tau| {
            let min_k = pairs
                .iter()
                .filter(|(_, agree)| *agree >= tau)
                .map(|(k, _)| *k)
                .min();
            json!({"tau": tau, "min_k": min_k})
        })
        .collect()
}
