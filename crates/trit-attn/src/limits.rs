//! `trit-attn` CLI 的参数上界（#593 / #594）。
//!
//! `recall_experiment` 会为每条 pair 建两个 `dim` 长的向量，并建 `dim×dim` 记忆；
//! 密集层权重 + 梯度各一份 `rows*cols` f32。无上界时直接触发分配器 abort。

/// `demo --pairs` 上限。
pub const MAX_PAIRS: usize = 1 << 16;
/// `demo --dim` 上限。
pub const MAX_DIM: usize = 1 << 12;
/// `train --hidden` 上限。
pub const MAX_HIDDEN: usize = 1 << 12;

/// 校验 `demo --pairs/--dim`。
pub fn check_demo(pairs: usize, dim: usize) -> Result<(), String> {
    if pairs > MAX_PAIRS {
        return Err(format!("--pairs 上限 {MAX_PAIRS}（本次 {pairs}）"));
    }
    if dim > MAX_DIM {
        return Err(format!("--dim 上限 {MAX_DIM}（本次 {dim}）"));
    }
    Ok(())
}

/// 校验 `train --hidden`。
pub fn check_hidden(hidden: usize) -> Result<(), String> {
    if hidden > MAX_HIDDEN {
        return Err(format!("--hidden 上限 {MAX_HIDDEN}（本次 {hidden}）"));
    }
    Ok(())
}
