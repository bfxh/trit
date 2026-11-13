//! 三值条件求值（从 sigscan.rs 拆出，控制单文件行数预算）：Kleene AND/OR/NOT
//! + for-of 全集迭代；`Unknown` 是证据缺失，绝不折叠成假。

use super::resolve::{Resolved, resolve};
use super::{Condition, ForBound};
use trit_core::value::Ternary;
/// 求值上下文：扫描数据（filesize / uintN 的来源）+ 截断标志 +
/// for-of 循环的当前串绑定（occurrences 的下标）+ 入口点文件偏移
///（调用方从可执行头解析传入；`None` = 上下文缺失，entrypoint 弃权）+
/// PE 模块信息（`pe.*` 子集属性求值；`None` = 非 PE / 缺上下文，弃权）。
pub(super) struct EvalCtx<'d, 'p> {
    pub(super) data: &'d [u8],
    pub(super) truncated: bool,
    pub(super) binding: Option<usize>,
    pub(super) entrypoint: Option<u64>,
    pub(super) pe: Option<&'p crate::pe::PeInfo>,
}

impl EvalCtx<'_, '_> {
    pub(super) fn with_binding(&self, index: usize) -> EvalCtx<'_, '_> {
        EvalCtx {
            data: self.data,
            truncated: self.truncated,
            binding: Some(index),
            entrypoint: self.entrypoint,
            pe: self.pe,
        }
    }
}

/// Kleene 合取：F 吞一切；T∧T=T；其余 U。
fn kleene_and(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::False, _) | (_, Ternary::False) => Ternary::False,
        (Ternary::True, Ternary::True) => Ternary::True,
        _ => Ternary::Unknown,
    }
}

/// Kleene 析取：T 吞一切；F∨F=F；其余 U。
fn kleene_or(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
        (Ternary::False, Ternary::False) => Ternary::False,
        _ => Ternary::Unknown,
    }
}

/// Kleene 取反：U 不塌缩。
fn kleene_not(v: Ternary) -> Ternary {
    match v {
        Ternary::True => Ternary::False,
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
    }
}

fn bool3(b: bool) -> Ternary {
    if b { Ternary::True } else { Ternary::False }
}

/// 条件求值（三值 Kleene；基于每模式的命中偏移表；操作数解析三态
/// `Resolved`/`resolve` 在 [`super::resolve`]——**确定的未定义 ≠ 证据
/// 缺失**，弃权绝不折叠成假）。
/// `True` = 判真；`False` = 完整证据下的判假；`Unknown` = 证据不足
///（不可求值的操作数 / 循环体弃权）——绝不把弃权折叠成假。
pub(super) fn eval(
    condition: &Condition,
    occurrences: &[(String, Vec<u64>)],
    ctx: &EvalCtx,
) -> Ternary {
    let set = |prefix: &Option<String>| -> Vec<usize> {
        occurrences
            .iter()
            .enumerate()
            .filter(|(_, (id, _))| prefix.as_deref().is_none_or(|p| id.starts_with(p)))
            .map(|(i, _)| i)
            .collect()
    };
    match condition {
        Condition::All => bool3(occurrences.iter().all(|(_, offs)| !offs.is_empty())),
        Condition::Any => bool3(occurrences.iter().any(|(_, offs)| !offs.is_empty())),
        Condition::Count(n) => {
            bool3(occurrences.iter().filter(|(_, o)| !o.is_empty()).count() >= *n)
        }
        Condition::At { index, offset } => {
            let Some((_, offs)) = occurrences.get(*index) else {
                return bool3(false); // 串下标越界：恒假
            };
            if offs.is_empty() {
                return bool3(false); // 零命中：无论 offset 是什么都不可在
            }
            match resolve(offset, occurrences, ctx) {
                Resolved::Val(v) => bool3(offs.contains(&v)),
                // 确定的未定义（如零命中串的 @a[1]）：串不可能"在"那里
                Resolved::DefiniteFalse => Ternary::False,
                Resolved::Unknown => Ternary::Unknown, // 证据缺失：弃权
            }
        }
        Condition::In { index, lo, hi } => bool3(
            occurrences
                .get(*index)
                .is_some_and(|(_, offs)| offs.iter().any(|o| o >= lo && o <= hi)),
        ),
        Condition::Ref(index) => bool3(
            occurrences
                .get(*index)
                .is_some_and(|(_, offs)| !offs.is_empty()),
        ),
        Condition::Cmp { lhs, op, rhs } => {
            match (
                resolve(lhs, occurrences, ctx),
                resolve(rhs, occurrences, ctx),
            ) {
                (Resolved::Val(a), Resolved::Val(b)) => bool3(op.apply(a, b)),
                // 与"确定的未定义"比较 = 假（官方语义：任何比较都不成立）
                (Resolved::DefiniteFalse, _) | (_, Resolved::DefiniteFalse) => Ternary::False,
                _ => Ternary::Unknown,
            }
        }
        Condition::ForOf {
            bound,
            prefix,
            body,
        } => for_of(*bound, prefix, body, occurrences, ctx),
        Condition::CurrentRef => bool3(
            ctx.binding
                .and_then(|i| occurrences.get(i))
                .is_some_and(|(_, offs)| !offs.is_empty()),
        ),
        Condition::CurrentAt { offset } => {
            let Some((_, offs)) = ctx.binding.and_then(|i| occurrences.get(i)) else {
                return bool3(false);
            };
            if offs.is_empty() {
                return bool3(false);
            }
            match resolve(offset, occurrences, ctx) {
                Resolved::Val(v) => bool3(offs.contains(&v)),
                Resolved::DefiniteFalse => Ternary::False,
                Resolved::Unknown => Ternary::Unknown,
            }
        }
        Condition::CurrentIn { lo, hi } => bool3(
            ctx.binding
                .and_then(|i| occurrences.get(i))
                .is_some_and(|(_, offs)| offs.iter().any(|o| o >= lo && o <= hi)),
        ),
        Condition::Const(b) => bool3(*b),
        Condition::And(conds) => conds
            .iter()
            .map(|c| eval(c, occurrences, ctx))
            .fold(Ternary::True, kleene_and),
        Condition::Or(conds) => conds
            .iter()
            .map(|c| eval(c, occurrences, ctx))
            .fold(Ternary::False, kleene_or),
        Condition::Not(inner) => kleene_not(eval(inner, occurrences, ctx)),
        Condition::OfAny { prefix } => {
            bool3(set(prefix).iter().any(|i| !occurrences[*i].1.is_empty()))
        }
        Condition::OfAll { prefix } => {
            let set = set(prefix);
            bool3(!set.is_empty() && set.iter().all(|i| !occurrences[*i].1.is_empty()))
        }
        Condition::OfCount { n, prefix } => bool3(
            set(prefix)
                .iter()
                .filter(|i| !occurrences[**i].1.is_empty())
                .count()
                >= *n,
        ),
        // 完整表达式树由 xeval 求值；旧求值器只如实弃权。
        Condition::Expr(_) => Ternary::Unknown,
    }
}

/// for-of 求值：对集合中**每条串**求体（官方 YARA 语义，差分测试实测
/// 判定：未命中串也在迭代里——`#`=0、`$`=假、`@`=弃权；`for all of
/// ($a*) : (# > 2)` 的惯用法正依赖全集迭代）。体弃权（Unknown）不折叠
/// 成假——N/any 在「已满足数不足且弃权可能补足」时如实上报 Unknown；
/// `all` 有弃权即 Unknown。集合里一条串都没有（空前缀族）= 无从满足。
fn for_of(
    bound: ForBound,
    prefix: &Option<String>,
    body: &Condition,
    occurrences: &[(String, Vec<u64>)],
    ctx: &EvalCtx,
) -> Ternary {
    let members: Vec<usize> = occurrences
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| prefix.as_deref().is_none_or(|p| id.starts_with(p)))
        .map(|(i, _)| i)
        .collect();
    if members.is_empty() {
        return bool3(false); // 空前缀族：没有可迭代的串
    }
    let total = members.len();
    let mut satisfied = 0usize;
    let mut abstained = 0usize;
    for i in members {
        match eval(body, occurrences, &ctx.with_binding(i)) {
            Ternary::True => satisfied += 1,
            Ternary::Unknown => abstained += 1,
            Ternary::False => {}
        }
    }
    match bound {
        ForBound::N(n) => {
            if satisfied >= n {
                Ternary::True
            } else if satisfied + abstained >= n {
                Ternary::Unknown
            } else {
                Ternary::False
            }
        }
        ForBound::Any => {
            if satisfied >= 1 {
                Ternary::True
            } else if abstained >= 1 {
                Ternary::Unknown
            } else {
                Ternary::False
            }
        }
        ForBound::All => {
            if satisfied == total {
                Ternary::True
            } else if abstained > 0 {
                Ternary::Unknown
            } else {
                Ternary::False
            }
        }
    }
}
