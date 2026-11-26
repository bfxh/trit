//! 常量回折判据：B3 的 `!(a & X)` 在化简后才出现 `Not(Const U)`，回折必须把它收成一条常量。
use crate::direct::emit;
use trit_core::{Algebra, Parser};
mod true_const;
#[test]
fn refold_closes_not_of_collapsed_constant() {
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
