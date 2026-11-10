use super::*;
use crate::bitplan;
use trit_core::Parser;

const ALGEBRAS: [Algebra; 3] = [Algebra::Kleene, Algebra::Lukasiewicz, Algebra::Bochvar];

#[test]
fn every_registered_rule_passes_self_check_in_kleene() {
    let active = active_rules(Algebra::Kleene);
    assert!(active.len() >= 15, "K3 规则集太窄：{active:?}");
    for name in [
        "de-morgan-and",
        "factor-and-or",
        "implies-elim",
        "equiv-elim",
    ] {
        assert!(active.contains(&name), "K3 缺少 {name}：{active:?}");
    }
    assert!(
        !active.contains(&"xor-elim"),
        "Xor 是强加法，不能按布尔异或消去"
    );
}

#[test]
fn loader_filters_rules_by_algebra() {
    // 蕴含消去在 K3 成立；Ł3 的蕴含是 1-a+b，`a=>b ≡ !a|b` 不成立（U=>U=1 vs U|U=U）。
    assert!(active_rules(Algebra::Kleene).contains(&"implies-elim"));
    assert!(!active_rules(Algebra::Lukasiewicz).contains(&"implies-elim"));
    // 等价消去反过来：`equiv` 本身就是这么定义的（`(a⇒b) ∧ (b⇒a)`），所以三种语义都过。
    for alg in ALGEBRAS {
        assert!(
            active_rules(alg).contains(&"equiv-elim"),
            "{alg:?} 应保留 equiv-elim"
        );
    }
}

#[test]
fn broken_rule_fails_self_check() {
    // 排中律在 K3 不成立：U | !U = U ≠ 1。
    let lhs = Expr::Or(
        Box::new(Expr::Var("a".into())),
        Box::new(Expr::Not(Box::new(Expr::Var("a".into())))),
    );
    let rhs = Expr::Const(Ternary::True);
    assert!(!self_check(&lhs, &rhs, Algebra::Kleene));
    // 同样的式子在三值上不成立、不过滤就进图的话，重写会静默改变语义。
    assert!(!self_check(&lhs, &rhs, Algebra::Lukasiewicz));
}

#[test]
fn factoring_reduces_node_count() {
    let expr = Parser::parse("(a & b) | (a & c)").unwrap();
    let (rewritten, fired) = rewrite(&expr, Algebra::Kleene, 32);
    assert_eq!(rewritten.to_infix(), "(a ∧ (b ∨ c))", "fired={fired:?}");
    assert!(fired.contains(&"factor-and-or"), "fired={fired:?}");
}

#[test]
fn rewrite_preserves_semantics_for_every_algebra() {
    for source in [
        "(a & b) | (a & c)",
        "!!(a | b)",
        "!(a & b) | c",
        "(a => b) & (b => a)",
        "(a ^ b) | (b & c)",
        "a & (a | b)",
        "((a & b) | (a & c)) & !d",
    ] {
        let expr = Parser::parse(source).unwrap();
        let vars = expr.variables();
        for alg in ALGEBRAS {
            let (rewritten, _) = rewrite(&expr, alg, 32);
            assert_eq!(
                bitplan::table(&rewritten, &vars, alg, 1).unwrap(),
                bitplan::table(&expr, &vars, alg, 1).unwrap(),
                "{source} @ {alg:?} 重写后语义变了：{source} → {}",
                rewritten.to_infix()
            );
        }
    }
}

#[test]
fn rewrite_is_deterministic_and_stable() {
    let expr = Parser::parse("((a & b) | (a & c)) & !(b & c)").unwrap();
    let (first, _) = rewrite(&expr, Algebra::Kleene, 32);
    let (again, _) = rewrite(&expr, Algebra::Kleene, 32);
    assert_eq!(
        first.to_infix(),
        again.to_infix(),
        "同一输入两次结果必须相同"
    );
    let (second_pass, _) = rewrite(&first, Algebra::Kleene, 32);
    assert_eq!(first.to_infix(), second_pass.to_infix(), "重写必须是固定点");
}

#[test]
fn saturation_stops_at_a_fixed_point() {
    let expr = Parser::parse("(((a & b) | (a & c)) & !(b & c)) | (a & b)").unwrap();
    let options = RewriteOptions::default();
    let first = rewrite_with(&expr, Algebra::Kleene, options);
    assert!(first.saturated, "默认上限内必须饱和：{first:?}");
    assert!(first.e_nodes > 0, "进图后应留下 e-class");
    assert!(first.rounds <= options.max_rounds);

    let second = rewrite_with(&first.expr, Algebra::Kleene, options);
    assert_eq!(first.expr.to_infix(), second.expr.to_infix());
    assert!(second.saturated);
    assert!(second.rounds <= 2, "固定点重跑最多再多一轮归一化");
}

#[test]
fn nodes_limit_marks_the_run_unsaturated() {
    let expr = Parser::parse("((a & b) | (a & c)) & ((b & c) | (b & d))").unwrap();
    let report = rewrite_with(
        &expr,
        Algebra::Kleene,
        RewriteOptions {
            node_limit: 1,
            ..RewriteOptions::default()
        },
    );
    assert!(!report.saturated, "触到节点上限不能声称饱和");
}

#[test]
fn cost_model_chooses_a_different_expression() {
    // A：5 个节点、2 个门；B：3 个节点、3 个门（等价/蕴含按多门算）。
    // 节点模型选 B，门模型选 A——这正是「有代价模型」与「只数节点」的差别。
    let a = Parser::parse("(a & b) & c").unwrap();
    let b = Parser::parse("a <=> b").unwrap();
    let mut graph = EGraph::new();
    let left = graph.add(&a);
    let right = graph.add(&b);
    graph.union(left, right);
    let (by_nodes, nodes_cost, _) = graph.extract(left, CostModel::Nodes);
    let (by_gates, gates_cost, _) = graph.extract(left, CostModel::Gates);
    assert_eq!(by_nodes.to_infix(), "(a ≡ b)", "节点数 3 < 5");
    assert_eq!(by_gates.to_infix(), "((a ∧ b) ∧ c)", "门数 2 < 3");
    assert_eq!(nodes_cost, 3);
    assert_eq!(gates_cost, 2);
}

#[test]
fn report_cost_matches_the_model() {
    let expr = Parser::parse("a => b").unwrap();
    for model in [CostModel::Nodes, CostModel::Gates] {
        let report = rewrite_with(
            &expr,
            Algebra::Kleene,
            RewriteOptions {
                cost: model,
                ..RewriteOptions::default()
            },
        );
        assert_eq!(
            (report.cost, report.nodes),
            expr_stats(&report.expr, model),
            "{model:?} 的报告代价必须与表达式重算一致"
        );
    }
}
