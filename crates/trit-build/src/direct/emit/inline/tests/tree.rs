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
    assert_eq!(source.matches("    int t").count(), 0, "{source}");
    assert!(source.contains("return T[3*T["), "{source}");
    assert!(source.contains("+v[2]+13];"), "{source}");
}
/// 单用锥整个并进根返回：`Not(And(Or(a,b),c))` 不留临时语句。
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
    assert_eq!(source.matches("    int t").count(), 0, "{source}");
    assert!(
        source.contains("return -T[3*T[3*v[0]+v[1]+13]+v[2]+4];"),
        "{source}"
    );
}
