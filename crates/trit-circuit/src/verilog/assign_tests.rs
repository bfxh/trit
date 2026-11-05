//! assign 右侧表达式的求值面：`eval_assign*` 助手 + 优先级 / 字面量 / 括号错误的钉。
//!
//! 从 `tests.rs` 拆出来——这一组只问"表达式怎么求值、怎么报错"，
//! 与同目录里那些向量端口、总线与层次连接的测试是两件事。公开面没有变化（全是测试）。

use super::*;
use trit_core::algebra::Algebra;
use trit_core::value::Ternary;

fn eval_assign(rhs: &str, values: &[(&str, Ternary)]) -> Ternary {
    eval_assign_with(rhs, values, Algebra::Kleene)
}

fn eval_assign_with(rhs: &str, values: &[(&str, Ternary)], alg: Algebra) -> Ternary {
    use crate::circuit::propagate;
    use std::collections::BTreeMap;
    let text = format!(
        "module m(input a, input b, input c, output y);
  assign y = {rhs};
endmodule
"
    );
    let circuit = parse_verilog(&text).unwrap();
    let mut assignment: BTreeMap<String, Ternary> = circuit
        .inputs
        .iter()
        .map(|signal| (signal.name().to_string(), Ternary::False))
        .collect();
    for (name, value) in values {
        assignment.insert(name.to_string(), *value);
    }
    let result = propagate(&circuit, &assignment, alg).unwrap();
    *result.signals.get("y").unwrap()
}

#[test]
fn assign_xor_is_classic_xor() {
    use trit_core::value::Ternary::*;
    assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", True)]), False);
    assert_eq!(eval_assign("a ^ b", &[("a", True), ("b", False)]), True);
    assert_eq!(eval_assign("a ~^ b", &[("a", True), ("b", True)]), True);
    assert_eq!(eval_assign("a ^~ b", &[("a", False), ("b", False)]), True);
    assert_eq!(
        eval_assign("a ^ b", &[("a", Unknown), ("b", True)]),
        Unknown
    );
    assert!(eval_assign("a ^ b", &[("a", Unknown), ("b", False)]).is_unknown());
}

#[test]
fn assign_xor_is_not_lukasiewicz_strong_sum() {
    use trit_core::value::Ternary::*;
    // Ł3 的强加 1 ⊕ 1 = 1，经典异或必须是 0。
    assert_eq!(
        eval_assign_with("a ^ b", &[("a", True), ("b", True)], Algebra::Lukasiewicz),
        False
    );
}

#[test]
fn assign_precedence_matches_verilog() {
    use trit_core::value::Ternary::*;
    // & 高于 ^ 高于 |
    assert_eq!(
        eval_assign("a ^ b & c", &[("a", True), ("b", True), ("c", False)]),
        True
    );
    assert_eq!(
        eval_assign("a & b | c", &[("a", True), ("b", False), ("c", False)]),
        False
    );
    assert_eq!(
        eval_assign("~(a | b) & c", &[("a", False), ("b", False), ("c", True)]),
        True
    );
}

#[test]
fn assign_literals_and_parentheses() {
    use trit_core::value::Ternary::*;
    assert_eq!(eval_assign("1'b1", &[]), True);
    assert_eq!(eval_assign("1'b0 | a", &[("a", False)]), False);
    assert_eq!(eval_assign("1'bx & a", &[("a", True)]), Unknown);
    assert_eq!(
        eval_assign("(a | b) & !c", &[("a", False), ("b", True), ("c", False)]),
        True
    );
}

#[test]
fn rejects_reversed_parentheses_without_panicking() {
    for text in [
        "module m;
and ) (;
endmodule
",
        "module m)(;
endmodule
",
        "module m;
and g1 (y, a) trailing;
endmodule
",
    ] {
        assert!(parse_verilog(text).is_err(), "{text}");
    }
}
