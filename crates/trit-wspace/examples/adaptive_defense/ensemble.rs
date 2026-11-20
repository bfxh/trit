//! 探针与集成的共享读数：响应签名、多数投票评测（从 main 拆出，守模块预算）。

use trit_wspace::Net;

use super::target::infer;

/// 探针响应向量（符号）。
pub(super) fn probe_response(net: &Net, probes: &[Vec<f32>]) -> Vec<i8> {
    probes
        .iter()
        .map(|x| {
            let v = infer(net, x);
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        })
        .collect()
}

/// 集成推理：成员多数投票（spam = 过半票）。
fn ensemble_infer(nets: &[Net], x: &[f32]) -> f32 {
    let votes: i32 = nets
        .iter()
        .map(|net| if infer(net, x) > 0.0 { 1 } else { -1 })
        .sum();
    if votes >= 0 { 1.0 } else { -1.0 }
}

/// 集成评测：(acc, spam-F1)。
pub(super) fn ensemble_evaluate(
    nets: &[Net],
    features: &[Vec<f32>],
    labels: &[usize],
    test: &[usize],
) -> (f32, f32) {
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_ = 0usize;
    for &i in test {
        let pred = usize::from(ensemble_infer(nets, &features[i]) > 0.0);
        match (pred, labels[i]) {
            (1, 1) => tp += 1,
            (1, 0) => fp += 1,
            (0, 1) => fn_ += 1,
            _ => {}
        }
    }
    let precision = tp as f32 / (tp + fp).max(1) as f32;
    let recall = tp as f32 / (tp + fn_).max(1) as f32;
    let f1 = 2.0 * precision * recall / (precision + recall).max(1e-12);
    let acc = (tp + fn_) as f32 / (tp + fp + fn_ + test.len().saturating_sub(tp + fp + fn_)) as f32;
    (acc, f1)
}
