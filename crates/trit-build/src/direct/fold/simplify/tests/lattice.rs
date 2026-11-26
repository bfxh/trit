//! 格吸收律判据：`x ∧ (x ∨ y) = x`、`x ∨ (x ∧ y) = x` 在 K3/Ł3 成立，B3 有反例。

use super::{lowered, simplified_alg};
use crate::direct::emit;
use crate::direct::ir::Op;
use trit_core::value::Ternary;
use trit_core::{Algebra, Parser};
mod chain;

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

/// 逐格对拍：K3/Ł3 的吸收律在 3×3 个值上都成立；B3 在 `F ≤ U` 处失败。
#[test]
fn lattice_absorption_holds_for_every_value_and_algebra() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        for x in VALUES {
            for y in VALUES {
                assert_eq!(alg.and(x, alg.or(x, y)), x, "{alg:?}");
                assert_eq!(alg.or(x, alg.and(x, y)), x, "{alg:?}");
            }
        }
    }
    assert_eq!(
        Algebra::Bochvar.and(
            Ternary::False,
            Algebra::Bochvar.or(Ternary::False, Ternary::Unknown)
        ),
        Ternary::Unknown
    );
}

/// 形状：内层节点包含外层另一操作数时，K3/Ł3 直接别名；B3 保留原式。
#[test]
fn lattice_absorption_aliases_to_the_operand() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        for source in ["a & (a | b)", "(a | b) & a", "a | (a & b)", "(a & b) | a"] {
            assert_eq!(
                simplified_alg(source, alg),
                vec![Op::Var(0)],
                "{source} {alg:?}"
            );
        }
    }
    for source in ["a & (a | b)", "a | (a & b)"] {
        assert_eq!(
            simplified_alg(source, Algebra::Bochvar),
            lowered(source),
            "{source} B3"
        );
    }
}

/// 反例：`a & (b | c)` 的内层不含 `a`，不能被吸收。
#[test]
fn non_absorbing_forms_stay() {
    for alg in ALGS {
        assert_eq!(simplified_alg("a & (b | c)", alg).len(), 5, "{alg:?}");
    }
}

/// 渲染面：K3/Ł3 吸收后只剩一条取变量语句，不注册任何算子表；B3 仍要表。
#[test]
fn absorbing_c_has_one_statement_and_no_tables() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        for source in ["a & (a | b)", "a | (a & b)"] {
            let expr = Parser::parse(source).expect("解析");
            let vars = expr.variables();
            let c = emit(&expr, &vars, alg).expect("生成");
            assert_eq!(
                c.matches("    int8_t t").count(),
                1,
                "{source} {alg:?}: {c}"
            );
            assert!(
                !c.contains("static const int8_t T_"),
                "{source} {alg:?}: {c}"
            );
        }
    }
    let expr = Parser::parse("a & (a | b)").expect("解析");
    let vars = expr.variables();
    let b3 = emit(&expr, &vars, Algebra::Bochvar).expect("生成");
    assert!(b3.contains("T_AND") && b3.contains("T_OR"), "{b3}");
}
