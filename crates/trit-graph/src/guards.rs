//! 分类器 / 消息传递路径的入口守卫：维度校验与数值有限性兜底。
//!
//! 错误口径：本仓不新增错误枚举/变体；维度不一致复用既有
//! [`trit_core::BusError::WidthMismatch`]。非有限数值按文档化钳制策略处理
//! （NaN/±inf → 中性 0.0），毒值不允许进入概率、损失、梯度或表项链。

use trit_core::value::Ternary;

use crate::view::TriGraphView;

/// 校验 `states` 长度与图节点数一致；长度不符返回既有
/// [`BusError::WidthMismatch`]（`left = states.len()`，`right = view.len()`）。
pub(crate) fn check_states_len<G: TriGraphView>(
    view: &G,
    states: &[Ternary],
) -> Result<(), trit_core::BusError> {
    if states.len() != view.len() {
        return Err(trit_core::BusError::WidthMismatch {
            left: states.len(),
            right: view.len(),
        });
    }
    Ok(())
}

/// 非有限 latent（NaN/±inf）按中性 0.0 参与软前向与梯度。
///
/// 钳制后 `tanh(0.0) = 0.0`，落在硬票死区内按弃权处理，与未知/中立口径一致；
/// 毒值不允许经 `tanh → 聚合 → 头 → softmax` 把概率、损失、梯度一起染 NaN。
pub(crate) fn finite_latent(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// 非有限标量（学习率等）按 0.0 处理：0 步长即不更新，不把毒值写回表项。
pub(crate) fn finite_scalar(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// 表 latent 梯度：3 边状态 × 3 节点状态。
pub(crate) type GradTable = [[f32; 3]; 3];
/// 头梯度：2 行（系数/偏置）× 3 类。
pub(crate) type GradHead = [[f32; 3]; 2];

/// 有限性守卫后的 softmax。
///
/// 标准 max-shift 只防有限大数的 `exp` 上溢，挡不住 NaN/±inf：任一槽位非有限
/// 先钳到中性 0.0，再做平移归一化。全 `-inf` 时 `inf - inf = NaN` 的分支
/// 同样在钳制后消失；输出三路概率均为有限值，和为 1。
pub(crate) fn softmax(logits: [f32; 3]) -> [f32; 3] {
    let logits = logits.map(finite_latent);
    let max = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp: [f32; 3] = logits.map(|value| (value - max).exp());
    let sum: f32 = exp.iter().sum();
    exp.map(|value| value / sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_handles_non_finite_logits() {
        let cases = [
            [f32::NAN, 0.0, -1.0],
            [f32::INFINITY, 1.0, 0.0],
            [f32::NEG_INFINITY, 1.0, 0.0],
            [f32::NEG_INFINITY; 3],
            [f32::NAN, f32::INFINITY, f32::NEG_INFINITY],
        ];
        for logits in cases {
            let probabilities = softmax(logits);
            assert!(
                probabilities.iter().all(|p| p.is_finite()),
                "logits={logits:?} probs={probabilities:?}"
            );
            let sum: f32 = probabilities.iter().sum();
            assert!((sum - 1.0).abs() < 1e-5, "sum={sum}");
        }
    }
}
