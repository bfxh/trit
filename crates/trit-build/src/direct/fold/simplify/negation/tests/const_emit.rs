//! 端到端判据：化简内的 `Not(Const)` 必须让根节点直接落常量，不再依赖后续 pass。

use crate::direct::emit;
use trit_core::{Algebra, Parser};

#[test]
fn constant_not_sheds_tables_in_bochvar_only() {
    let expr = Parser::parse("!(a & X)").expect("解析");
    let vars = expr.variables();
    let b3 = emit(&expr, &vars, Algebra::Bochvar).expect("生成");
    assert_eq!(b3.matches("    int8_t t").count(), 1, "{b3}");
    assert!(!b3.contains("T_NOT"), "{b3}");
    assert!(!b3.contains("T_AND"), "{b3}");

    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        let c = emit(&expr, &vars, alg).expect("生成");
        assert!(c.contains("T_NOT"), "{alg:?}: {c}");
        assert!(c.contains("T_AND"), "{alg:?}: {c}");
    }
}

#[test]
fn k3_not_of_true_constant_folds_to_false() {
    let expr = Parser::parse("!(a | 1)").expect("解析");
    let vars = expr.variables();
    let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert_eq!(c.matches("    int8_t t").count(), 1, "{c}");
    assert!(c.contains("int8_t t0 = -1"), "{c}");
    assert!(!c.contains("T_NOT"), "{c}");
}
