//! B3 毒值吸收判据：`x ∧ U = U`、`x ∨ U = U` 只在 B3 成立。

use super::{lowered, simplified_alg};
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::value::Ternary;

/// B3 的 `U` 沾上就传染；K3/Ł3 的 `F ∧ U = F`、`T ∨ U = T` 必须保留原式。
#[test]
fn poison_absorbs_and_or_only_in_bochvar() {
    for source in ["a & X", "a | X", "X & a", "X | a"] {
        assert_eq!(
            simplified_alg(source, Algebra::Bochvar),
            vec![Op::Const(Ternary::Unknown)],
            "{source}"
        );
        for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
            assert_eq!(
                simplified_alg(source, alg),
                lowered(source),
                "{alg:?} {source}"
            );
        }
    }
}
