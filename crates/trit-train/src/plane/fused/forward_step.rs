//! 单样本 SGD 的逐行融合：前向 `q·x`、MSE/`dy`、`dW` 更新与 scale 更新共用一行。
//!
//! 旧路径是 `fused::forward_dot` 扫一遍 latent，`dense::sgd_update` 再扫一遍；
//! 这里按行先算 `qdot`，立即得到 `dy` 并更新该行，行数据留在缓存里，也不再分配
//! `y` / `dy` 两个临时向量。`qdot`、loss、`dW` 与 scale 的算术顺序同旧路径。

use crate::dense;
use crate::quant::Ste;

/// 单样本 SGD 一步，返回更新前的 MSE；逐位口径同 `TernaryLinear::train_step`。
#[allow(clippy::too_many_arguments)]
pub fn train_step(
    latent: &mut [f32],
    scale: &mut [f32],
    rows: usize,
    cols: usize,
    x: &[f32],
    target: &[f32],
    lr: f32,
    ste: Ste,
) -> f32 {
    assert_eq!(latent.len(), rows * cols, "latent 长度必须是 rows * cols");
    assert_eq!(scale.len(), rows, "scale 长度必须是 rows");
    assert_eq!(x.len(), cols, "输入长度必须是 cols");
    assert_eq!(target.len(), rows, "目标长度必须是 rows");
    let mut loss = 0.0f32;
    for r in 0..rows {
        let s = scale[r];
        let row = &mut latent[r * cols..(r + 1) * cols];
        let qdot = super::forward_dot_row(row, s, cols, x);
        let diff = s * qdot - target[r];
        loss += 0.5 * diff * diff;
        dense::sgd_update_row(row, x, diff * s, lr, s, ste);
        scale[r] = (s - lr * diff * qdot).max(1e-4);
    }
    loss / rows as f32
}

#[cfg(test)]
mod tests;
