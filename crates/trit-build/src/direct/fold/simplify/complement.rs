//! B3 的补元消去：`x ∧ (¬x ∨ y) = x ∧ y`、`x ∨ (¬x ∧ y) = x ∨ y`。
//!
//! K3/Ł3 在 `x = U` 处有反例：`U ∧ (U ∨ F) = U`，右边 `U ∧ F = F`；
//! 对偶式 `U ∨ (U ∧ T) = U`，右边 `U ∨ T = T`。只有 B3 的 `U` 传染能让
//! 两边都等于 `U`，所以守卫是 `Algebra::Bochvar`。

use trit_core::Algebra;

use crate::direct::ir::Op;

/// 返回（被补元的操作数，应与它组合的另一个操作数）；不匹配或非 B3 返回 `None`。
pub(super) fn pair(
    alg: Algebra,
    kept: &[Op],
    left: usize,
    right: usize,
    inner_or: bool,
) -> Option<(usize, usize)> {
    if !matches!(alg, Algebra::Bochvar) {
        return None;
    }
    if let Some(pair) = complement_operand(kept, left, right, inner_or) {
        return Some(pair);
    }
    complement_operand(kept, right, left, inner_or)
}

fn complement_operand(
    kept: &[Op],
    outer: usize,
    inner: usize,
    inner_or: bool,
) -> Option<(usize, usize)> {
    let (first, second) = match (inner_or, kept[inner]) {
        (true, Op::Or(a, b)) | (false, Op::And(a, b)) => (a, b),
        _ => return None,
    };
    if is_complement(kept, outer, first) {
        return Some((outer, second));
    }
    if is_complement(kept, outer, second) {
        return Some((outer, first));
    }
    None
}

fn is_complement(kept: &[Op], outer: usize, inner: usize) -> bool {
    match kept[outer] {
        Op::Not(base) => base == inner,
        _ => matches!(kept[inner], Op::Not(base) if base == outer),
    }
}
