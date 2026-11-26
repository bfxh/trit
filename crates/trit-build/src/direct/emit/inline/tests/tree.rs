//! 普通二元树的单用锥：不再只有 `Not` 能并句。

use super::*;

#[test]
fn single_use_binary_tree_collapses_into_root() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::And(0, 1),
            Op::Var(2),
            Op::Or(2, 3),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 0, "{source}");
    assert!(source.contains("return T_OR[(T_AND["), "{source}");
    assert!(source.contains("+ 1) * 3 + (vars[2] + 1)];"), "{source}");
}
/// 三层以内的单用锥整个并进根返回：`Not(And(Or(a,b),c))` 不留临时语句。
#[test]
fn multilevel_single_use_cone_fits_one_statement() {
    let source = render(
        vec![
            Op::Var(0),
            Op::Var(1),
            Op::Or(0, 1),
            Op::Var(2),
            Op::And(2, 3),
            Op::Not(4),
        ],
        &vars(),
        Algebra::Kleene,
    );
    assert_eq!(source.matches("    int8_t t").count(), 0, "{source}");
    assert!(
        source.contains(
            "return -(T_AND[(T_OR[(vars[0] + 1) * 3 + (vars[1] + 1)] + 1) * 3 + (vars[2] + 1)]);"
        ),
        "{source}"
    );
}
