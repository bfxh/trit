//! 交换律规范化与死代码消除的判据：表对称性、镜像子式共用、孤儿节点压缩。

use super::*;
use trit_core::{Algebra, Parser};

const VALUES: [Ternary; 3] = [Ternary::False, Ternary::Unknown, Ternary::True];

type BinaryFn = fn(Algebra, Ternary, Ternary) -> Ternary;

/// 允许按交换律重排的四个算子，在三种代数下都必须满足 `f(a,b) == f(b,a)`。
#[test]
fn commutative_operators_are_symmetric_in_all_algebras() {
    let operators: [(&str, BinaryFn); 4] = [
        ("&", Algebra::and),
        ("|", Algebra::or),
        ("^", Algebra::strong_add),
        ("<=>", Algebra::equiv),
    ];
    for alg in Algebra::ALL {
        for (name, function) in operators {
            for left in VALUES {
                for right in VALUES {
                    assert_eq!(
                        function(alg, left, right),
                        function(alg, right, left),
                        "{alg:?} 的 {name} 在 {left:?}/{right:?} 上不对称"
                    );
                }
            }
        }
    }
}

/// `(a & b) | (b & a)`：规范化后两个与式共用同一个节点。
#[test]
fn mirrored_subexpressions_share_one_node() {
    let expr = Parser::parse("(a & b) | (b & a)").expect("解析");
    let vars = expr.variables();
    let ops = lower(&expr, &vars).expect("降级");
    assert_eq!(ops.len(), 4, "镜像子式没有共用：{ops:?}");
    assert_eq!(
        ops.iter().filter(|op| matches!(op, Op::And(..))).count(),
        1,
        "{ops:?}"
    );
    assert_eq!(ops[2], Op::And(0, 1), "{ops:?}");
}

/// 子节点按 IR 下标排序：源里写 `b & a`，IR 里仍是 `And(a, b)`。
#[test]
fn operands_are_ordered_by_ir_index() {
    let vars = vec!["a".to_string(), "b".to_string()];
    for (source, expected) in [
        ("b & a", Op::And(0, 1)),
        ("b | a", Op::Or(0, 1)),
        ("b ^ a", Op::Xor(0, 1)),
        ("b <=> a", Op::Equiv(0, 1)),
    ] {
        let expr = Parser::parse(source).expect("解析");
        let ops = lower(&expr, &vars).expect("降级");
        assert_eq!(ops.last(), Some(&expected), "{source}: {ops:?}");
    }
}

/// 合成 IR 里的孤儿节点（第 3 条 `Const(True)`）被丢掉，后继下标重编号。
#[test]
fn dead_nodes_are_pruned_and_indices_remapped() {
    let ops = vec![
        Op::Const(Ternary::False),
        Op::Var(0),
        Op::And(0, 1),
        Op::Const(Ternary::True),
        Op::Not(2),
    ];
    let kept = prune_dead(ops, 4);
    assert_eq!(kept.len(), 4, "{kept:?}");
    assert_eq!(kept[3], Op::Not(2), "{kept:?}");
    assert!(
        !kept.contains(&Op::Const(Ternary::True)),
        "孤儿节点没被删：{kept:?}"
    );
}

/// 当前 lowering 不产生孤儿：DCE 对真实表达式是恒等变换。
#[test]
fn lowering_has_no_dead_nodes() {
    for source in ["a & b | !c", "((a => b) <=> (!a | b)) ^ c", "1", "a"] {
        let expr = Parser::parse(source).expect("解析");
        let vars = expr.variables();
        let ops = lower(&expr, &vars).expect("降级");
        assert_eq!(prune_dead(ops.clone(), ops.len() - 1), ops, "{source}");
    }
}
