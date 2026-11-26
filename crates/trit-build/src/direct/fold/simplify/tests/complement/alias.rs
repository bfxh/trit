//! 补元消去的别名边界：`y` 就是 `x` 时退化成幂等律，必须别名而不是新建节点。

use super::super::simplified_alg;
use crate::direct::ir::Op;
use trit_core::Algebra;

#[test]
fn complement_other_equal_to_outer_aliases() {
    for source in ["a & (!a | a)", "a | (!a & a)"] {
        assert_eq!(
            simplified_alg(source, Algebra::Bochvar),
            vec![Op::Var(0)],
            "{source}"
        );
    }
}
