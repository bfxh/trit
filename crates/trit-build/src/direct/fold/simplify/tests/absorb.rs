//! 吸收律的代数分派判据：K3/Ł3 落常量，B3 保留原式。

use super::{lowered, simplified_alg};
use crate::direct::emit;
use crate::direct::ir::Op;
use trit_core::Algebra;
use trit_core::Parser;
use trit_core::value::Ternary;

/// 吸收律只在 K3/Ł3 落常量；B3 保留原式（`U & F = U` / `U | T = U`）。
#[test]
fn absorption_is_only_for_k3_and_lukasiewicz() {
    for alg in [Algebra::Kleene, Algebra::Lukasiewicz] {
        assert_eq!(
            simplified_alg("a & 0", alg),
            vec![Op::Const(Ternary::False)],
            "{alg:?}"
        );
        assert_eq!(
            simplified_alg("a | 1", alg),
            vec![Op::Const(Ternary::True)],
            "{alg:?}"
        );
    }
    assert_eq!(simplified_alg("a & 0", Algebra::Bochvar), lowered("a & 0"));
    assert_eq!(simplified_alg("a | 1", Algebra::Bochvar), lowered("a | 1"));
}

/// 吸收律的渲染差异：K3 直接返回常量，B3 仍要算子表。
#[test]
fn absorbing_c_depends_on_the_algebra() {
    for source in ["a & 0", "a | 1"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let k3 = emit(&expr, &vars, Algebra::Kleene).expect("生成");
        assert_eq!(k3.matches("int t").count(), 0, "{source}: {k3}");
        assert!(!k3.contains("C T[]") && k3.contains("return "));

        let b3 = emit(&expr, &vars, Algebra::Bochvar).expect("生成");
        assert!(b3.contains("T["), "{source} B3 应保留算子表: {b3}");
        assert!(b3.contains("v[0]"), "{source}: {b3}");
    }
}
