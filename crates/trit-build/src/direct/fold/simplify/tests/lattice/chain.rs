//! 格吸收的链判据：K3/Ł3 的 `x ∧ (x ∨ y ∨ z) = x`、`x ∨ (x ∧ y ∧ z) = x`；
//! 同算子链递归找 `x`，异算子处必须停手，B3 由毒值守卫整条拦下。

use super::super::{lowered, simplified_alg};
use crate::direct::emit;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::Parser;
use trit_core::value::Ternary;

const ALGS: [Algebra; 2] = [Algebra::Kleene, Algebra::Lukasiewicz];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

/// 3×3×3 逐格对拍：K3/Ł3 的链吸收成立；B3 的 `F` 反例仍在。
#[test]
fn chain_absorption_holds_for_k3_and_lukasiewicz() {
    for alg in ALGS {
        for x in VALUES {
            for y in VALUES {
                for z in VALUES {
                    assert_eq!(
                        alg.and(x, alg.or(alg.or(x, y), z)),
                        x,
                        "{alg:?} {x:?} {y:?} {z:?}"
                    );
                    assert_eq!(
                        alg.or(x, alg.and(alg.and(x, y), z)),
                        x,
                        "{alg:?} {x:?} {y:?} {z:?}"
                    );
                }
            }
        }
    }
    assert_eq!(
        Algebra::Bochvar.and(
            Ternary::False,
            Algebra::Bochvar.or(
                Algebra::Bochvar.or(Ternary::False, Ternary::Unknown),
                Ternary::True
            )
        ),
        Ternary::Unknown
    );
}

/// 形状：三层左链、右嵌套链与对偶式都直接别名到 `x`。
#[test]
fn chain_absorption_aliases_to_the_operand() {
    for alg in ALGS {
        for source in [
            "a & (a | b | c)",
            "(a | b | c) & a",
            "a & (b | (c | a))",
            "a | (a & b & c)",
            "(a & b & c) | a",
        ] {
            assert_eq!(
                simplified_alg(source, alg),
                vec![Op::Var(0)],
                "{source} {alg:?}"
            );
        }
    }
}

/// 反例：`a` 埋在异算子 `a & c` 里时不能跨算子吸收。
#[test]
fn cross_operator_nesting_stays() {
    let source = "a & (b | (a & c))";
    assert_eq!(
        simplified_alg(source, Algebra::Kleene),
        lowered(source),
        "{source}"
    );
}

/// B3 不适用链吸收：毒值让 `F ∧ (F ∨ U ∨ T) = U`，IR 保留原式。
#[test]
fn b3_chain_forms_stay() {
    let source = "a & (a | b | c)";
    assert_eq!(
        simplified_alg(source, Algebra::Bochvar),
        lowered(source),
        "{source}"
    );
}

/// 渲染面：K3 链吸收后根节点直接返回变量、不注册算子表；B3 仍要两张表。
#[test]
fn chain_absorbing_c_drops_tables() {
    for source in ["a & (a | b | c)", "a | (a & b & c)"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
        assert_eq!(c.matches("int t").count(), 0, "{source}: {c}");
        assert!(!c.contains("C T[]"), "{source}: {c}");
    }
    let expr = Parser::parse("a & (a | b | c)").expect("解析");
    let vars = expr.variables();
    let b3 = emit(&expr, &vars, Algebra::Bochvar).expect("生成");
    assert!(b3.contains("C T[]"), "{b3}");
}
