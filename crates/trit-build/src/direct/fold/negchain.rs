//! 德摩根链归一：`¬(¬a ∧ ¬b ∧ ¬c) = a ∨ b ∨ c`，对偶式落 `And` 链；只接三叶以上，
//! 两叶仍由 `simplify` 回收。命中要求 `Not` 直接盖住同算子链且每叶都是 `Not`，任一
//! 叶子不是 `Not` 就整棵保留，不猜「子式里含 ¬x」。必须在化简前跑，否则内层
//! `And(¬a, ¬b)` 先被收成 `Or(a, b)`，外层就看不出原链。三代数都成立：K3/Ł3 是
//! 格序 min/max，B3 的 U 在两侧同值时仍满足德摩根。

use crate::direct::ir::Op;
use std::collections::HashSet;

pub(super) fn collapse(ops: Vec<Op>) -> Vec<Op> {
    if ops.is_empty() {
        return ops;
    }
    let mut remap = vec![usize::MAX; ops.len()];
    let mut kept = Vec::with_capacity(ops.len());
    for (index, op) in ops.iter().enumerate() {
        match *op {
            Op::Not(inner) => {
                if let Some((result_and, leaves)) = chain(&ops, inner) {
                    let mut bases = leaves.into_iter().map(|leaf| remap[leaf]);
                    let first = bases.next().expect("链至少三个叶子");
                    let root = bases.fold(first, |left, right| {
                        let op = if result_and {
                            Op::And(left, right)
                        } else {
                            Op::Or(left, right)
                        };
                        push(&mut kept, op)
                    });
                    remap[index] = root;
                    continue;
                }
                let inner = remap[inner];
                remap[index] = push(&mut kept, Op::Not(inner));
            }
            other => remap[index] = push(&mut kept, other.remap(&remap)),
        }
    }
    crate::direct::lower::prune_dead(kept, remap[remap.len() - 1])
}

/// 返回（结果是否用 `And`，叶子基操作数）；外层 `Not` 盖 `And` 落 `Or`，
/// 盖 `Or` 落 `And`。叶子少于三个或任一叶子不是 `Not` 就返回 `None`。
fn chain(ops: &[Op], inner: usize) -> Option<(bool, Vec<usize>)> {
    let (want_or, result_and) = match ops.get(inner)? {
        Op::And(..) => (false, false),
        Op::Or(..) => (true, true),
        _ => return None,
    };
    let mut leaves = Vec::new();
    let mut seen = HashSet::new();
    if !collect(ops, inner, want_or, &mut leaves, &mut seen) || leaves.len() < 3 {
        return None;
    }
    Some((result_and, leaves))
}

fn collect(
    ops: &[Op],
    index: usize,
    want_or: bool,
    leaves: &mut Vec<usize>,
    seen: &mut HashSet<usize>,
) -> bool {
    match ops[index] {
        Op::Not(base) => {
            leaves.push(base);
            true
        }
        Op::And(a, b) if !want_or => {
            if !seen.insert(index) {
                return true;
            }
            collect(ops, a, want_or, leaves, seen) && collect(ops, b, want_or, leaves, seen)
        }
        Op::Or(a, b) if want_or => {
            if !seen.insert(index) {
                return true;
            }
            collect(ops, a, want_or, leaves, seen) && collect(ops, b, want_or, leaves, seen)
        }
        _ => false,
    }
}

fn push(kept: &mut Vec<Op>, op: Op) -> usize {
    kept.push(op);
    kept.len() - 1
}

#[cfg(test)]
mod tests;
