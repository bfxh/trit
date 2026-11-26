//! 格吸收的同算子链传递：`x ∧ (x ∨ y ∨ z) = x`、`x ∨ (x ∧ y ∧ z) = x`。
//!
//! 只沿同一算子（`Or` 沿 `Or`、`And` 沿 `And`）下钻；遇到异算子节点就停，
//! 不做「子式里含 x」这种分配律级别的猜测。K3/Ł3 是格序 min/max，链上每个
//! 析取项都 ≥ x（合取项都 ≤ x），所以传递吸收成立；B3 由 `lattice` 的守卫拦下。

use std::collections::HashSet;

use crate::direct::ir::Op;

pub(super) fn contains(kept: &[Op], index: usize, needle: usize, inner_or: bool) -> bool {
    let mut stack = vec![index];
    let mut seen = HashSet::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        let (a, b) = match (inner_or, kept[node]) {
            (true, Op::Or(a, b)) | (false, Op::And(a, b)) => (a, b),
            _ => continue,
        };
        if a == needle || b == needle {
            return true;
        }
        stack.push(a);
        stack.push(b);
    }
    false
}
