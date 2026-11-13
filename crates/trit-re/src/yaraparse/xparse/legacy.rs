//! 旧解析器的"记名弃权"探测：旧条件解析器把认不出的模块路径 / 函数
//! （`pe.number_of_imports`、`uint16be(0)`、`pe.imports("a", "b")`）收成
//! `Operand::Opaque`，求值恒 `Unknown`。这类规则旧解析器"接住了"却永远
//! 给不出结论——表达式引擎能求值就该让它来（只增证据面：`Unknown` →
//! 确定值；旧解析器真能求值的路径一条不动）。
//!
//! 下面的清单必须与 `sigscan/{peprops,sections,rich}.rs` 的求值子集一致
//! （`tests/yara_expr_route.rs` 逐条钉死：清单内的路径在旧引擎上都能
//! 给出确定结论）。

use super::lex::T;
use crate::sigscan::{CmpOp, Condition, Operand};

const SCALARS: &[&str] = &[
    "pe.machine",
    "pe.number_of_sections",
    "pe.timestamp",
    "pe.characteristics",
    "pe.entry_point",
    "pe.overlay.size",
    "pe.overlay.offset",
    "pe.is_dll(…)",
    "pe.is_32bit(…)",
];
const SECTION: &[&str] = &[
    "characteristics",
    "virtual_size",
    "virtual_address",
    "raw_size",
    "raw_ptr",
    "name",
];
const RICH: &[&str] = &[
    "version",
    "key",
    "offset",
    "length",
    "clear_data",
    "raw_data",
];

/// 旧求值器对这条路径有没有实现（有上下文时能给出值或确定的未定义）。
pub(super) fn resolves(path: &str) -> bool {
    if let Some(rest) = path.strip_prefix("pe.sections[") {
        return rest.split_once("].").is_some_and(|(index, field)| {
            !index.is_empty()
                && index.bytes().all(|b| b.is_ascii_digit())
                && SECTION.contains(&field)
        });
    }
    match path.strip_prefix("pe.rich_signature.") {
        Some(field) => RICH.contains(&field),
        None => SCALARS.contains(&path),
    }
}

fn dead(operand: &Operand) -> bool {
    matches!(operand, Operand::Opaque(path) if !resolves(path))
}

/// 旧求值器按字符串处理的操作数（`sigscan/strval.rs` 的接管面）。
fn stringy(operand: &Operand) -> bool {
    match operand {
        Operand::LitStr(_) => true,
        Operand::Opaque(path) => {
            (path.starts_with("pe.sections[") && path.ends_with(".name"))
                || path == "pe.rich_signature.clear_data"
                || path == "pe.rich_signature.raw_data"
        }
        _ => false,
    }
}

/// 旧求值器只会字符串的 `==` / `!=`：字符串序（`.name < .name`——解析层
/// 拦不住两个路径相比）和"字符串比数值"它恒弃权。
fn unordered(lhs: &Operand, op: CmpOp, rhs: &Operand) -> bool {
    let (left, right) = (stringy(lhs), stringy(rhs));
    (left || right) && !(left && right && matches!(op, CmpOp::Eq | CmpOp::Ne))
}

/// 条件里有没有旧求值器永远求不出值的操作数 / 比较。
pub(super) fn abstains_by_name(condition: &Condition) -> bool {
    match condition {
        Condition::At { offset, .. } | Condition::CurrentAt { offset } => dead(offset),
        Condition::Cmp { lhs, op, rhs } => dead(lhs) || dead(rhs) || unordered(lhs, *op, rhs),
        Condition::ForOf { body, .. } => abstains_by_name(body),
        Condition::Not(inner) => abstains_by_name(inner),
        Condition::And(items) | Condition::Or(items) => items.iter().any(abstains_by_name),
        _ => false,
    }
}

/// 这个操作数官方可能求成"未定义"（越界读、没有的第 N 次命中、缺失的
/// 模块字段）。
fn may_be_undefined(operand: &Operand) -> bool {
    matches!(
        operand,
        Operand::UintRead { .. }
            | Operand::Offset { .. }
            | Operand::CurrentOffset { .. }
            | Operand::Entrypoint
            | Operand::Opaque(_)
    )
}

fn has_undefined(condition: &Condition) -> bool {
    match condition {
        Condition::At { offset, .. } | Condition::CurrentAt { offset } => may_be_undefined(offset),
        Condition::Cmp { lhs, rhs, .. } => may_be_undefined(lhs) || may_be_undefined(rhs),
        Condition::ForOf { body, .. } => has_undefined(body),
        Condition::Not(inner) => has_undefined(inner),
        Condition::And(items) | Condition::Or(items) => items.iter().any(has_undefined),
        _ => false,
    }
}

/// 条件里有没有 `not` 罩着可能未定义的比较。旧求值器把"确定的未定义"
/// 记成假再取反得真；官方的 `not 未定义` 仍是未定义（规则不命中）——
/// 差分实测 `not (uint8(越界) == 1)`：官方 miss，旧引擎 HIT。这类条件
/// 交给表达式引擎（它带着未定义走完整棵树）。
pub(super) fn negates_undefined(condition: &Condition) -> bool {
    match condition {
        Condition::Not(inner) => has_undefined(inner),
        Condition::ForOf { body, .. } => negates_undefined(body),
        Condition::And(items) | Condition::Or(items) => items.iter().any(negates_undefined),
        _ => false,
    }
}

/// 模块数组的字面量下标超出 C `int`：官方把下标截成 32 位再用
/// （`pe.sections[4294967296]` 就是第 0 节——探针实测命中），旧求值器按
/// "越界 = 确定未定义"判假，与官方有 DIFF。表达式引擎照官方截断。
/// `@a[N]` 不在此列（串偏移的下标官方不截断）。
pub(super) fn wraps_index(tokens: &[T]) -> bool {
    tokens.windows(4).any(|window| {
        matches!(
            window,
            [T::Ident(_), T::P("["), T::Int(index), T::P("]")] if *index > i64::from(i32::MAX)
        )
    })
}

/// 旧解析器收下了 `for … of` 套 `for … of`——官方是编译错误
/// （"'for <quantifier> of <string set>' loops can't be nested"）。
pub(super) fn nests_string_loops(condition: &Condition) -> bool {
    fn walk(condition: &Condition, inside: bool) -> bool {
        match condition {
            Condition::ForOf { body, .. } => inside || walk(body, true),
            Condition::Not(inner) => walk(inner, inside),
            Condition::And(items) | Condition::Or(items) => {
                items.iter().any(|item| walk(item, inside))
            }
            _ => false,
        }
    }
    walk(condition, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_legacy_paths() {
        for path in [
            "pe.machine",
            "pe.is_dll(…)",
            "pe.sections[0].name",
            "pe.sections[12].raw_size",
        ] {
            assert!(resolves(path), "{path}");
        }
        assert!(resolves("pe.rich_signature.key"));
        for path in [
            "pe.number_of_imports",
            "pe.sections[…].name",
            "pe.sections[0].raw_data_size",
            "pe.sections[].name",
            "pe.imports(…)",
            "uint16be(…)",
            "math.entropy(…)",
            "pe.rich_signature.toolid(…)",
        ] {
            assert!(!resolves(path), "{path}");
        }
    }

    #[test]
    fn spots_negated_possibly_undefined_comparisons() {
        let read = || Condition::Cmp {
            lhs: Operand::UintRead { size: 1, offset: 9 },
            op: crate::sigscan::CmpOp::Eq,
            rhs: Operand::Lit(1),
        };
        let not = |inner| Condition::Not(Box::new(inner));
        assert!(negates_undefined(&not(read())));
        assert!(negates_undefined(&Condition::And(vec![
            Condition::Ref(0),
            not(not(read()))
        ])));
        assert!(negates_undefined(&not(Condition::Or(vec![
            read(),
            Condition::Const(false)
        ]))));
        // 没有 not，或 not 底下全是恒有定义的量：旧引擎照旧
        assert!(!negates_undefined(&read()));
        assert!(!negates_undefined(&not(Condition::Ref(0))));
        let sized = Condition::Cmp {
            lhs: Operand::Filesize,
            op: crate::sigscan::CmpOp::Lt,
            rhs: Operand::Count(0),
        };
        assert!(!negates_undefined(&not(sized)));
    }
}
