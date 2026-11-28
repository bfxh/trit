//! 格运算对偶：`(p ∨ q) ⊕ (p ∧ q) = p ⊕ q`，`⊕` 为强加或经典 XOR。
//! 三代数 3×3 逐格成立；只认同一对 `p/q` 的 `Or` 与 `And` 直接操作数。

use crate::direct::ir::Op;

pub(crate) fn meet_join(kept: &[Op], left: usize, right: usize) -> Option<(usize, usize)> {
    match (kept[left], kept[right]) {
        (Op::Or(p, q), Op::And(r, s)) | (Op::And(p, q), Op::Or(r, s)) => {
            if (p == r && q == s) || (p == s && q == r) {
                return Some((p, q));
            }
            None
        }
        _ => None,
    }
}
