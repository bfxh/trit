use std::collections::HashSet;

use trit_bdd::{BddError, BddLimits, BddRequest, count_models_bdd};
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_eval::{Assignment, eval};

fn parse(source: &str) -> Expr {
    Parser::parse(source).expect("test expression must parse")
}

fn names(list: &[&str]) -> Vec<String> {
    list.iter().map(|name| (*name).to_string()).collect()
}

/// 独立参考实现：直接在 {False,True,Unknown} 上枚举。
fn brute(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> u128 {
    let domains: Vec<Vec<Ternary>> = vars
        .iter()
        .map(|name| {
            if binary.contains(name) {
                vec![Ternary::False, Ternary::True]
            } else {
                vec![Ternary::False, Ternary::True, Ternary::Unknown]
            }
        })
        .collect();
    let sizes: Vec<usize> = domains.iter().map(|domain| domain.len()).collect();
    let mut index = vec![0usize; vars.len()];
    let mut count = 0u128;
    loop {
        let mut assignment = Assignment::new();
        for (slot, name) in vars.iter().enumerate() {
            assignment.set(name.clone(), domains[slot][index[slot]]);
        }
        let holds = constraints
            .iter()
            .all(|constraint| eval(constraint, &assignment, alg) == Ternary::True);
        if holds && eval(expr, &assignment, alg) == target {
            count += 1;
        }
        let mut place = 0;
        loop {
            if place == vars.len() {
                return count;
            }
            index[place] += 1;
            if index[place] < sizes[place] {
                break;
            }
            index[place] = 0;
            place += 1;
        }
    }
}

fn count(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> u128 {
    count_models_bdd(
        expr,
        constraints,
        vars,
        binary,
        alg,
        target,
        &BddRequest::default(),
    )
    .expect("BDD counting")
}

#[test]
fn counts_match_brute_force_across_algebras() {
    let sources = [
        "a",
        "!a",
        "a & b",
        "a | b",
        "a ^ b",
        "a => b",
        "a <=> b",
        "(a & b) | c",
        "!(a | b) ^ c",
    ];
    for alg in Algebra::ALL {
        for source in sources {
            let expr = parse(source);
            let vars = names(&["a", "b", "c"]);
            let empty = HashSet::new();
            let one_binary: HashSet<String> = ["a"].iter().map(|n| n.to_string()).collect();
            for constraints in [vec![], vec![parse("a | c")], vec![parse("!(a & b)")]] {
                for binary in [&empty, &one_binary] {
                    for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
                        let bdd = count(&expr, &constraints, &vars, binary, alg, target);
                        let reference = brute(&expr, &constraints, &vars, binary, alg, target);
                        assert_eq!(
                            bdd,
                            reference,
                            "alg={} expr={source} constraints={:?} binary={:?} target={target:?}",
                            alg.as_str(),
                            constraints.len(),
                            binary.len(),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn unused_variables_are_still_counted() {
    let expr = parse("a & b");
    let vars = names(&["a", "b", "c"]);
    let empty = HashSet::new();
    for alg in Algebra::ALL {
        let total: u128 = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| count(&expr, &[], &vars, &empty, alg, target))
            .sum();
        assert_eq!(total, 27, "{}", alg.as_str());
    }
}

#[test]
fn constant_expression_spans_the_declared_domain() {
    let truth = parse("1");
    let empty = HashSet::new();
    let ternary = names(&["a", "b"]);
    let binary: HashSet<String> = ["a", "b"].iter().map(|n| n.to_string()).collect();
    for alg in Algebra::ALL {
        assert_eq!(count(&truth, &[], &ternary, &empty, alg, Ternary::True), 9);
        assert_eq!(count(&truth, &[], &ternary, &empty, alg, Ternary::False), 0);
        assert_eq!(count(&truth, &[], &ternary, &binary, alg, Ternary::True), 4);
        assert_eq!(
            count(&truth, &[], &ternary, &binary, alg, Ternary::Unknown),
            0
        );
        // 只引用 a 的二值表达式：b 完全不出现在 BDD 里，仍要按 2 计。
        let only_a = parse("a");
        assert_eq!(
            count(&only_a, &[], &ternary, &binary, alg, Ternary::True),
            2
        );
        assert_eq!(
            count(&only_a, &[], &ternary, &binary, alg, Ternary::False),
            2
        );
    }
}

#[test]
fn constraints_filter_the_domain() {
    let expr = parse("a & b");
    let vars = names(&["a", "b"]);
    let constraints = vec![parse("a")];
    let empty = HashSet::new();
    for alg in Algebra::ALL {
        let counts: Vec<u128> = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| count(&expr, &constraints, &vars, &empty, alg, target))
            .collect();
        assert_eq!(counts, vec![1, 1, 1], "{}", alg.as_str());
    }
}

#[test]
fn node_budget_still_rejects() {
    let expr = parse("a & b");
    let vars = names(&["a", "b"]);
    let empty = HashSet::new();
    let request = BddRequest {
        order: None,
        limits: BddLimits { max_nodes: 1 },
    };
    let error = count_models_bdd(
        &expr,
        &[],
        &vars,
        &empty,
        Algebra::Kleene,
        Ternary::True,
        &request,
    )
    .expect_err("one node cannot hold the validity BDD");
    assert!(
        matches!(error, BddError::BudgetExceeded { .. }),
        "unexpected error: {error}"
    );
}

#[test]
fn counts_beyond_u128_are_rejected_not_wrapped() {
    let truth = parse("1");
    let vars: Vec<String> = (0..200).map(|index| format!("v{index}")).collect();
    let empty = HashSet::new();
    let error = count_models_bdd(
        &truth,
        &[],
        &vars,
        &empty,
        Algebra::Kleene,
        Ternary::True,
        &BddRequest::default(),
    )
    .expect_err("3^200 does not fit in u128");
    assert!(
        matches!(error, BddError::CountTooLarge),
        "unexpected error: {error}"
    );
}
