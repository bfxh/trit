//! B3 补元消去判据：`x ∧ (¬x ∨ y) = x ∧ y`、`x ∨ (¬x ∧ y) = x ∨ y`；
//! K3/Ł3 在 `x = U` 处有反例，必须保留原式。

use super::{lowered, simplified_alg};
use crate::direct::emit;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::Parser;
use trit_core::value::Ternary;
mod alias;

const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

/// B3 的 U 传染让两条补元消去在 3×3 个值上都成立。
#[test]
fn complement_elimination_holds_for_b3() {
    let alg = Algebra::Bochvar;
    for x in VALUES {
        for y in VALUES {
            assert_eq!(
                alg.and(x, alg.or(alg.not(x), y)),
                alg.and(x, y),
                "{x:?} {y:?}"
            );
            assert_eq!(
                alg.or(x, alg.and(alg.not(x), y)),
                alg.or(x, y),
                "{x:?} {y:?}"
            );
        }
    }
}

/// K3/Ł3 的 `x = U` 反例：左边分别落 U、U，右边落 F、T。
#[test]
fn complement_elimination_stays_for_k3_and_lukasiewicz() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        let x = Ternary::Unknown;
        assert_ne!(
            alg.and(x, alg.or(alg.not(x), Ternary::False)),
            alg.and(x, Ternary::False),
            "{alg:?}"
        );
        assert_ne!(
            alg.or(x, alg.and(alg.not(x), Ternary::True)),
            alg.or(x, Ternary::True),
            "{alg:?}"
        );
    }
}

/// 四种形状都重建为 `x ∧ y` / `x ∨ y`，顺序不影响匹配。
#[test]
fn complement_elimination_aliases_both_orders() {
    for source in ["a & (!a | b)", "(!a | b) & a"] {
        assert_eq!(
            simplified_alg(source, Algebra::Bochvar),
            vec![Op::Var(0), Op::Var(1), Op::And(0, 1)],
            "{source}"
        );
    }
    for source in ["a | (!a & b)", "(!a & b) | a"] {
        assert_eq!(
            simplified_alg(source, Algebra::Bochvar),
            vec![Op::Var(0), Op::Var(1), Op::Or(0, 1)],
            "{source}"
        );
    }
}

/// IR 面把门关死：K3/Ł3 的四种形状一条语句都不能少。
#[test]
fn complement_elimination_is_b3_only_in_the_ir() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        for source in [
            "a & (!a | b)",
            "(!a | b) & a",
            "a | (!a & b)",
            "(!a & b) | a",
        ] {
            assert_eq!(
                simplified_alg(source, alg),
                lowered(source),
                "{source} {alg:?}"
            );
        }
    }
}

/// 内层不是补元时不能消去。
#[test]
fn non_complement_forms_stay() {
    assert_eq!(
        simplified_alg("a & (!b | c)", Algebra::Bochvar).len(),
        6,
        "a & (!b | c)"
    );
}

/// 渲染面：B3 消去后不再注册 `T_NOT` / `T_OR`，只剩 `T_AND`；对偶式只剩 `T_OR`。
#[test]
fn complement_elimination_drops_redundant_tables() {
    let expr = Parser::parse("a & (!a | b)").expect("解析");
    let vars = expr.variables();
    let c = emit(&expr, &vars, Algebra::Bochvar).expect("生成");
    assert_eq!(c.matches("    int8_t t").count(), 3, "{c}");
    assert!(!c.contains("T_NOT") && !c.contains("T_OR"), "{c}");
    assert!(c.contains("T_AND"), "{c}");

    let expr = Parser::parse("a | (!a & b)").expect("解析");
    let vars = expr.variables();
    let c = emit(&expr, &vars, Algebra::Bochvar).expect("生成");
    assert_eq!(c.matches("    int8_t t").count(), 3, "{c}");
    assert!(!c.contains("T_NOT") && !c.contains("T_AND"), "{c}");
    assert!(c.contains("T_OR"), "{c}");
}
