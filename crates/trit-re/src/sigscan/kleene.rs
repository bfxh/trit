//! Kleene 三值逻辑原语（从 `eval.rs` 拆出守预算）。

use trit_core::value::Ternary;

/// Kleene 合取：F 吞一切；T∧T=T；其余 U。
pub(super) fn kleene_and(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::False, _) | (_, Ternary::False) => Ternary::False,
        (Ternary::True, Ternary::True) => Ternary::True,
        _ => Ternary::Unknown,
    }
}

/// Kleene 析取：T 吞一切；F∨F=F；其余 U。
pub(super) fn kleene_or(a: Ternary, b: Ternary) -> Ternary {
    match (a, b) {
        (Ternary::True, _) | (_, Ternary::True) => Ternary::True,
        (Ternary::False, Ternary::False) => Ternary::False,
        _ => Ternary::Unknown,
    }
}

/// Kleene 取反：U 不塌缩。
pub(super) fn kleene_not(v: Ternary) -> Ternary {
    match v {
        Ternary::True => Ternary::False,
        Ternary::False => Ternary::True,
        Ternary::Unknown => Ternary::Unknown,
    }
}

pub(super) fn bool3(b: bool) -> Ternary {
    if b { Ternary::True } else { Ternary::False }
}
