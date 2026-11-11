//! 三值对应：`levels = 2` 的链与 `trit-core` 的 Kleene 弱运算逐点对拍。

use trit_core::algebra::Algebra;
use trit_core::value::Ternary;
use trit_mv::{Chain, TNorm};

fn to_ternary(level: u32) -> Ternary {
    match level {
        0 => Ternary::False,
        1 => Ternary::Unknown,
        _ => Ternary::True,
    }
}

#[test]
fn weak_ops_match_kleene() {
    let chain = Chain::new(2).unwrap();
    let kleene = Algebra::Kleene;
    for x in chain.values() {
        for y in chain.values() {
            let tx = to_ternary(x);
            let ty = to_ternary(y);
            assert_eq!(
                to_ternary(chain.weak_and(x, y).unwrap()),
                kleene.and(tx, ty),
                "weak_and x={x} y={y}"
            );
            assert_eq!(
                to_ternary(chain.weak_or(x, y).unwrap()),
                kleene.or(tx, ty),
                "weak_or x={x} y={y}"
            );
        }
        assert_eq!(
            to_ternary(chain.neg(x).unwrap()),
            kleene.not(to_ternary(x)),
            "neg x={x}"
        );
    }
}

#[test]
fn lukasiewicz_three_chain_matches_known_tables() {
    let chain = Chain::new(2).unwrap();
    // 强合取：U ⊗ U = F（这是 Ł3 与 Kleene 的关键差别）。
    assert_eq!(chain.t_norm(TNorm::Lukasiewicz, 1, 1).unwrap(), 0);
    assert_eq!(chain.t_norm(TNorm::Lukasiewicz, 2, 1).unwrap(), 1);
    assert_eq!(chain.t_norm(TNorm::Lukasiewicz, 2, 2).unwrap(), 2);
    // 强析取：U ⊕ U = T。
    assert_eq!(chain.t_conorm(TNorm::Lukasiewicz, 1, 1).unwrap(), 2);
    // 蕴含：U → U = T。
    assert_eq!(chain.residuum(TNorm::Lukasiewicz, 1, 1).unwrap(), 2);
}
