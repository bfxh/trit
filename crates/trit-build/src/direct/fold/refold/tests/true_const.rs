//! 非 `U` 的常量回折：K3 的 `!(a | 1)` 化简后是 `Not(Const T)`，必须折成 `F`。

use super::*;

#[test]
fn refold_folds_not_of_true_constant() {
    let expr = Parser::parse("!(a | 1)").expect("解析");
    let vars = expr.variables();
    let c = emit(&expr, &vars, Algebra::Kleene).expect("生成");
    assert_eq!(c.matches("    int8_t t").count(), 1, "{c}");
    assert!(c.contains("int8_t t0 = -1"), "{c}");
    assert!(!c.contains("T_NOT"), "{c}");
}
