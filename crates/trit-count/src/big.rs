//! `--big` 的 DPLL vs BDD 对拍（拆出以守住 `main.rs` 行数预算）。

use super::*;

/// DPLL vs BDD 对拍（不跑穷举），用于 n>12 的第三方校验。
pub fn run(cases: usize, max_n: usize) -> ExitCode {
    let mut rng = SeedRng::new(0x5EED_1234_ABCD_EF01);
    let mut failures = 0usize;
    let mut total = 0usize;
    let mut refused = 0usize;
    let mut overflowed = 0usize;
    for n in (13..=max_n).step_by(4) {
        let vars: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
        let mut max_dpll = 0.0f64;
        let mut max_bdd = 0.0f64;
        for _ in 0..cases {
            for alg in Algebra::ALL {
                for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
                    let expr = random_expr(&mut rng, &vars, 4);
                    let start = Instant::now();
                    let dpll_result =
                        count_models_under(&expr, &[], &vars, &HashSet::new(), alg, target);
                    let dpll = dpll_result.count;
                    let dpll_ms = start.elapsed().as_secs_f64() * 1e3;

                    let start = Instant::now();
                    let bdd = count_models_bdd(
                        &expr,
                        &[],
                        &vars,
                        &HashSet::new(),
                        alg,
                        target,
                        CROSSCHECK_BDD_LIMITS,
                    );
                    let bdd_ms = start.elapsed().as_secs_f64() * 1e3;
                    total += 1;
                    match bdd {
                        Ok(count) => {
                            if dpll_result.stats.overflows > 0 {
                                // 饱和值是下界（`u128::MAX`），不可与 BDD 精确值对拍；
                                // 如实计数，不当成 MISMATCH（#591）。
                                overflowed += 1;
                            } else if count != dpll {
                                failures += 1;
                                eprintln!(
                                    "MISMATCH n={n} alg={} target={target:?} dpll={dpll} bdd={count} expr={expr:?}",
                                    alg.as_str(),
                                );
                            }
                        }
                        Err(_) => refused += 1,
                    }
                    max_dpll = max_dpll.max(dpll_ms);
                    max_bdd = max_bdd.max(bdd_ms);
                }
            }
        }
        println!(
            "n={n:2} cases={cases:2} max_dpll={max_dpll:8.3}ms max_bdd={max_bdd:8.3}ms refused={refused}"
        );
    }
    println!(
        "big_total={total} failures={failures} bdd_refused={refused} dpll_overflowed={overflowed}"
    );
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
