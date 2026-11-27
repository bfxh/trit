//! 幂等律：`x ∧ x = x`、`x ∨ x = x` 在 K3/Ł3/B3 都成立（与 Ł3 的强合取 `⊗` 区分）。

use super::{lowered, simplified_alg};
use crate::direct::emit;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::Parser;
use trit_core::value::Ternary;

const ALGS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];
const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

/// 逐格对拍：三种代数 × 三个值都必须满足 `x ∧ x = x`、`x ∨ x = x`。
#[test]
fn idempotence_holds_for_every_value_and_algebra() {
    for alg in ALGS {
        for value in VALUES {
            assert_eq!(alg.and(value, value), value, "{alg:?}");
            assert_eq!(alg.or(value, value), value, "{alg:?}");
        }
    }
}

/// 形状：两个操作数指向同一节点时，结果别名到该节点，不新增语句。
#[test]
fn idempotence_aliases_to_the_operand() {
    for alg in ALGS {
        for source in ["a & a", "a | a"] {
            assert_eq!(
                simplified_alg(source, alg),
                vec![Op::Var(0)],
                "{source} {alg:?}"
            );
        }
        assert_eq!(
            simplified_alg("(a & b) | (a & b)", alg),
            lowered("a & b"),
            "{alg:?}"
        );
    }
}

/// 渲染面：幂等律消掉算子后根节点直接返回变量，不注册任何算子表。
#[test]
fn idempotent_c_has_one_statement_and_no_tables() {
    for alg in ALGS {
        for source in ["a & a", "a | a"] {
            let expr = Parser::parse(source).expect("解析");
            let vars = expr.variables();
            let c = emit(&expr, &vars, alg).expect("生成");
            assert_eq!(c.matches("int t").count(), 0, "{source} {alg:?}: {c}");
            assert!(
                !c.contains("static const int8_t T[]"),
                "{source} {alg:?}: {c}"
            );
        }
    }
}
