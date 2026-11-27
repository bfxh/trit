//! 格吸收律：`x ∧ (x ∨ y) = x`、`x ∨ (x ∧ y) = x`。
//!
//! K3/Ł3 的 `∧` / `∨` 是格序 min/max，吸收成立；B3 的 `U` 传染会破坏它：
//! `F ∧ (F ∨ U) = F ∧ U = U`，所以 B3 不做这条规则。

use trit_core::Algebra;

use crate::direct::ir::Op;
mod chain;

pub(super) fn absorbed(
    alg: Algebra,
    kept: &[Op],
    left: usize,
    right: usize,
    inner_or: bool,
) -> Option<usize> {
    if matches!(alg, Algebra::Bochvar) {
        return None;
    }
    if chain::contains(kept, right, left, inner_or) {
        return Some(left);
    }
    if chain::contains(kept, left, right, inner_or) {
        return Some(right);
    }
    None
}
