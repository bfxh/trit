//! 格吸收律：`x ∧ (x ∨ y) = x`、`x ∨ (x ∧ y) = x`。
//! K3/Ł3 成立；B3 的 `U` 传染破坏它，不做。

use trit_core::Algebra;

use crate::direct::ir::Op;
mod chain;
pub(super) mod pair;

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
