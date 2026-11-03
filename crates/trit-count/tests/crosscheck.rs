use std::collections::HashSet;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_count::{
    count_models, count_models_bdd, count_models_under, crosscheck, crosscheck_under,
    crosscheck_under_with_limits,
};

fn parse(source: &str) -> Expr {
    Parser::parse(source).expect("test expression must parse")
}

fn vars(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

#[test]
fn fixed_expressions_match_exhaustive() {
    let expressions = [
        "a",
        "!a",
        "a & b",
        "a | b",
        "a ^ b",
        "a => b",
        "a <=> b",
        "(a => b) <=> (!a | b)",
        "(a ^ b) & (a | b)",
        "a & X",
        "a | X",
        "!(a & b) | (c ^ d)",
        "(a => (b => c)) <=> ((a & b) => c)",
    ];
    let names = ["a", "b", "c", "d"];
    let vars = vars(&names);
    let binary = HashSet::new();

    for alg in Algebra::ALL {
        for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
            for source in expressions {
                let expr = parse(source);
                let report = crosscheck(&expr, &vars, &binary, alg, target);
                assert!(
                    report.matches,
                    "mismatch: alg={} target={target:?} expr={source} dpll={} exhaustive={}",
                    alg.as_str(),
                    report.dpll_count,
                    report.exhaustive_count
                );
            }
        }
    }
}

#[test]
fn target_partition_is_total_domain() {
    let vars = vars(&["a", "b", "c"]);
    let binary = HashSet::new();
    let expressions = ["a & b", "a | X", "(a => b) ^ (b <=> c)", "!a", "a & !a"];
    for alg in Algebra::ALL {
        for source in expressions {
            let expr = parse(source);
            let total: u128 = [Ternary::False, Ternary::True, Ternary::Unknown]
                .into_iter()
                .map(|target| count_models(&expr, &vars, &binary, alg, target).count)
                .sum();
            assert_eq!(
                total,
                3u128.pow(vars.len() as u32),
                "partition failed: alg={} expr={source}",
                alg.as_str()
            );
        }
    }
}

#[test]
fn binary_variables_use_two_valued_domain() {
    let vars = vars(&["a", "b", "c"]);
    let binary: HashSet<String> = ["a", "b"].iter().map(|name| (*name).to_string()).collect();
    let expr = parse("(a & b) => c");
    for alg in Algebra::ALL {
        let total: u128 = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| count_models(&expr, &vars, &binary, alg, target).count)
            .sum();
        assert_eq!(
            total,
            2u128.pow(2) * 3,
            "binary partition failed for {}",
            alg.as_str()
        );
        for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
            let report = crosscheck(&expr, &vars, &binary, alg, target);
            assert!(report.matches, "binary mismatch: {}", alg.as_str());
        }
    }
}

#[test]
fn unused_variables_still_count() {
    let expr = parse("a & b");
    let vars = vars(&["a", "b", "c"]);
    for alg in Algebra::ALL {
        let total: u128 = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| count_models(&expr, &vars, &HashSet::new(), alg, target).count)
            .sum();
        assert_eq!(
            total,
            27,
            "unused variable not counted for {}",
            alg.as_str()
        );
    }
}

#[test]
fn random_n_up_to_eight_match_exhaustive() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for n in 1..=8usize {
        let vars: Vec<String> = (0..n).map(|index| format!("v{index}")).collect();
        for _ in 0..3 {
            for alg in Algebra::ALL {
                for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
                    let expr = random_expr(&mut rng, &vars, 4);
                    let report = crosscheck(&expr, &vars, &HashSet::new(), alg, target);
                    assert!(
                        report.matches,
                        "random mismatch n={n} alg={} target={target:?} dpll={} exhaustive={} bdd={:?}",
                        alg.as_str(),
                        report.dpll_count,
                        report.exhaustive_count,
                        report.bdd_count
                    );
                    assert!(
                        report.three_way,
                        "bdd engine absent n={n} alg={} error={:?}",
                        alg.as_str(),
                        report.bdd_error
                    );
                }
            }
        }
    }
}

#[test]
fn random_n_twelve_match_exhaustive() {
    let vars: Vec<String> = (0..12).map(|index| format!("v{index}")).collect();
    let mut rng = Rng(0x1234_5678_9ABC_DEF0);
    for alg in Algebra::ALL {
        for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
            let expr = random_expr(&mut rng, &vars, 4);
            let report = crosscheck(&expr, &vars, &HashSet::new(), alg, target);
            assert!(
                report.matches,
                "n=12 mismatch alg={} target={target:?} dpll={} exhaustive={} bdd={:?}",
                alg.as_str(),
                report.dpll_count,
                report.exhaustive_count,
                report.bdd_count
            );
            assert!(
                report.three_way,
                "bdd engine absent at n=12 alg={} error={:?}",
                alg.as_str(),
                report.bdd_error
            );
        }
    }
}

/// 穷举跑不动的规模上，DPLL 与 BDD 两条独立链仍要给出同一个数。
#[test]
fn dpll_and_bdd_agree_beyond_exhaustive() {
    let mut rng = Rng(0xB1B0_DDEA_D1CC_0001);
    for n in [13usize, 17, 21] {
        let vars: Vec<String> = (0..n).map(|index| format!("v{index}")).collect();
        for _ in 0..2 {
            for alg in Algebra::ALL {
                for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
                    let expr = random_expr(&mut rng, &vars, 4);
                    let dpll = count_models(&expr, &vars, &HashSet::new(), alg, target).count;
                    let bdd = count_models_bdd(
                        &expr,
                        &[],
                        &vars,
                        &HashSet::new(),
                        alg,
                        target,
                        trit_bdd::BddLimits::UNLIMITED,
                    )
                    .expect("unlimited bdd budget should count");
                    assert_eq!(dpll, bdd, "n={n} alg={} target={target:?}", alg.as_str());
                }
            }
        }
    }
}

/// 节点预算拦下 BDD 时是「这一路没出数」，不是三方不一致。
#[test]
fn bdd_budget_refusal_is_not_a_mismatch() {
    let vars: Vec<String> = (0..6).map(|index| format!("v{index}")).collect();
    let expr = random_expr(&mut Rng(0x0F1C_1CE5_5EED_1234), &vars, 4);
    let tiny = trit_bdd::BddLimits { max_nodes: 1 };
    let refused = count_models_bdd(
        &expr,
        &[],
        &vars,
        &HashSet::new(),
        Algebra::Kleene,
        Ternary::True,
        tiny,
    );
    assert!(
        matches!(refused, Err(trit_bdd::BddError::BudgetExceeded { .. })),
        "one node should not be enough: {refused:?}"
    );

    let report = crosscheck_under_with_limits(
        &expr,
        &[],
        &vars,
        &HashSet::new(),
        Algebra::Kleene,
        Ternary::True,
        tiny,
    );
    assert_eq!(report.bdd_count, None, "budget must refuse the bdd engine");
    assert!(report.bdd_error.is_some(), "a refusal needs a reason");
    assert!(
        report.matches,
        "dpll/exhaustive still agree: {} vs {}",
        report.dpll_count, report.exhaustive_count
    );
    assert!(
        !report.three_way,
        "a refused engine must not be counted as three-way agreement"
    );

    let room = crosscheck(
        &expr,
        &vars,
        &HashSet::new(),
        Algebra::Kleene,
        Ternary::True,
    );
    assert!(
        room.matches && room.three_way,
        "crosscheck budget fits: {:?}",
        room.bdd_error
    );
    assert_eq!(room.bdd_error, None);
}

#[test]
fn constant_expression_counts_whole_domain() {
    let vars: Vec<String> = (0..12).map(|index| format!("v{index}")).collect();
    let expr = parse("1");
    for alg in Algebra::ALL {
        let count = count_models(&expr, &vars, &HashSet::new(), alg, Ternary::True).count;
        assert_eq!(
            count,
            3u128.pow(12),
            "constant true count for {}",
            alg.as_str()
        );
        let count = count_models(&expr, &vars, &HashSet::new(), alg, Ternary::False).count;
        assert_eq!(count, 0, "constant false count for {}", alg.as_str());
    }
}

#[test]
fn component_product_multiplies_independent_parts() {
    let cnf = trit_count::cnf::Cnf {
        nvars: 4,
        clauses: vec![vec![1, 2], vec![3, 4]],
    };
    let result = trit_count::dpll::count_cnf(&cnf);
    assert_eq!(result.count, 9);
    assert!(result.stats.components > 0);
}

#[test]
fn positive_count_matches_trit_sat_satisfiable() {
    let mut rng = Rng(0x0BAD_F00D_DEAD_BEEF);
    for n in 1..=8usize {
        let vars: Vec<String> = (0..n).map(|index| format!("v{index}")).collect();
        for _ in 0..5 {
            for alg in Algebra::ALL {
                let expr = random_expr(&mut rng, &vars, 4);
                let count = count_models(&expr, &vars, &HashSet::new(), alg, Ternary::True).count;
                let witness =
                    trit_sat::satisfiable(&expr, alg, &HashSet::new()).expect("SAT query");
                assert_eq!(
                    count > 0,
                    witness.is_some(),
                    "positive-count vs SAT disagreement: alg={} count={count}",
                    alg.as_str()
                );
                if let Some(assignment) = witness {
                    assert_eq!(
                        trit_eval::eval(&expr, &assignment, alg),
                        Ternary::True,
                        "SAT witness does not evaluate to true"
                    );
                }
            }
        }
    }
}

#[test]
fn constrained_counts_match_exhaustive() {
    let vars = vars(&["a", "b", "c"]);
    let binary = HashSet::new();
    let cases = [
        ("a & b", vec!["a"]),
        ("a | b", vec!["!a", "c | !c"]),
        ("(a => b) ^ c", vec!["a", "!b"]),
        ("a & b", vec!["X"]),
    ];
    for alg in Algebra::ALL {
        for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
            for (source, constraint_sources) in &cases {
                let expr = parse(source);
                let constraints: Vec<Expr> =
                    constraint_sources.iter().map(|src| parse(src)).collect();
                let report = crosscheck_under(&expr, &constraints, &vars, &binary, alg, target);
                assert!(
                    report.matches,
                    "constrained mismatch: alg={} target={target:?} expr={source} constraints={constraint_sources:?} bdd={:?}",
                    alg.as_str(),
                    report.bdd_count
                );
                assert!(
                    report.three_way,
                    "constrained bdd engine absent: expr={source} error={:?}",
                    report.bdd_error
                );
            }
        }
    }
}

#[test]
fn constraints_restrict_domain() {
    let vars = vars(&["a", "b"]);
    let expr = parse("a & b");
    let constraint = parse("a");
    for alg in Algebra::ALL {
        let counts: Vec<u128> = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| {
                count_models_under(
                    &expr,
                    std::slice::from_ref(&constraint),
                    &vars,
                    &HashSet::new(),
                    alg,
                    target,
                )
                .count
            })
            .collect();
        assert_eq!(counts.iter().sum::<u128>(), 3, "{}", alg.as_str());
        assert_eq!(counts[1], 1, "{}", alg.as_str());
    }
}

#[test]
fn constraints_combine_and_cover_unused_variables() {
    let vars = vars(&["a", "b", "c"]);
    let expr = parse("a & b");
    let constraints = [parse("a"), parse("b")];
    for alg in Algebra::ALL {
        let total: u128 = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| {
                count_models_under(&expr, &constraints, &vars, &HashSet::new(), alg, target).count
            })
            .sum();
        assert_eq!(total, 3, "{}", alg.as_str());
    }

    let only_c = [parse("c")];
    for alg in Algebra::ALL {
        let total: u128 = [Ternary::False, Ternary::True, Ternary::Unknown]
            .into_iter()
            .map(|target| {
                count_models_under(&expr, &only_c, &vars, &HashSet::new(), alg, target).count
            })
            .sum();
        assert_eq!(total, 9, "{}", alg.as_str());
    }
}

fn random_expr(rng: &mut Rng, vars: &[String], depth: usize) -> Expr {
    if depth == 0 || rng.below(4) == 0 {
        return if rng.below(6) == 0 {
            Expr::Const(match rng.below(3) {
                0 => Ternary::False,
                1 => Ternary::True,
                _ => Ternary::Unknown,
            })
        } else {
            Expr::Var(vars[rng.below(vars.len())].clone())
        };
    }
    match rng.below(6) {
        0 => Expr::Not(Box::new(random_expr(rng, vars, depth - 1))),
        1 => Expr::and(
            random_expr(rng, vars, depth - 1),
            random_expr(rng, vars, depth - 1),
        ),
        2 => Expr::or(
            random_expr(rng, vars, depth - 1),
            random_expr(rng, vars, depth - 1),
        ),
        3 => Expr::xor(
            random_expr(rng, vars, depth - 1),
            random_expr(rng, vars, depth - 1),
        ),
        4 => Expr::implies(
            random_expr(rng, vars, depth - 1),
            random_expr(rng, vars, depth - 1),
        ),
        _ => Expr::equiv(
            random_expr(rng, vars, depth - 1),
            random_expr(rng, vars, depth - 1),
        ),
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit as u64) as usize
    }
}
