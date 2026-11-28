//! 否定归一：常量否定、双重否定消去 + 德摩根回收。
//!
//! `¬¬x = x` 三代数通用。`¬(¬a ∧ ¬b) = a ∨ b`、`¬(¬a ∨ ¬b) = a ∧ b`
//! 也三代数通用：K3/Ł3 的 `∧`/`∨` 是格序 min/max，B3 的 U 传染在两侧同值。
//! 二元形状由 `de_morgan` 回收，节点数 4→1；三叶以上的同算子链由 `negchain` 在化简前处理。
//! 单侧取反仍要新建 `Not`，不减少节点，保留原式。
//! `Not(Const)` 当场折成常量：否则父节点在本遍看不到这个毒值，会错过吸收。
mod de_morgan;
mod resolved;
pub(super) use self::resolved::push_resolved;
use crate::direct::ir::Op;
use trit_core::value::Ternary;

pub(super) fn push(
    resolve: &mut Vec<usize>,
    kept: &mut Vec<Op>,
    consts: &mut Vec<Option<Ternary>>,
    inner: usize,
) {
    let inner = resolve[inner];
    push_resolved(resolve, kept, consts, inner);
}

#[cfg(test)]
mod tests;
