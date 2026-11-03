use std::collections::HashSet;
use std::process::ExitCode;
use std::time::Instant;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_count::{
    CROSSCHECK_BDD_LIMITS, count_models_bdd, count_models_under, crosscheck, crosscheck_under,
    oracle::{self, count_exhaustive_under},
    policy::Policy,
};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        usage();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == "--big") {
        let max_n = match flag_value(&args, "--big") {
            None => 24,
            Some(value) if value.starts_with("--") => 24,
            Some(value) => match value.parse() {
                Ok(max_n) => max_n,
                Err(_) => {
                    eprintln!("--big 后面要跟个数：--big 24");
                    return ExitCode::from(2);
                }
            },
        };
        let cases = flag_value(&args, "--cases")
            .and_then(|value| value.parse().ok())
            .unwrap_or(2);
        return run_big(cases, max_n);
    }
    if args.iter().any(|arg| arg == "--random") {
        let cases = flag_value(&args, "--cases")
            .and_then(|value| value.parse().ok())
            .unwrap_or(2);
        return run_random(cases);
    }
    if let Some(path) = flag_value(&args, "--policy") {
        return run_policy(&path);
    }

    let Some(source) = flag_value(&args, "--expr") else {
        eprintln!("缺少 --expr；用 --help 看用法");
        return ExitCode::from(2);
    };
    let expr = match Parser::parse(&source) {
        Ok(expr) => expr,
        Err(error) => {
            eprintln!("解析失败: {error}");
            return ExitCode::from(2);
        }
    };
    let alg = flag_value(&args, "--alg")
        .as_deref()
        .and_then(Algebra::parse)
        .unwrap_or(Algebra::Kleene);
    let mut constraints = Vec::new();
    for source in flag_values(&args, "--constraint") {
        match Parser::parse(&source) {
            Ok(constraint) => constraints.push(constraint),
            Err(error) => {
                eprintln!("约束解析失败: {error}");
                return ExitCode::from(2);
            }
        }
    }
    let target = flag_value(&args, "--target")
        .as_deref()
        .map(parse_target)
        .transpose()
        .unwrap_or_else(|error| {
            eprintln!("{error}");
            std::process::exit(2);
        })
        .unwrap_or(Ternary::True);

    let mut vars: Vec<String> = flag_value(&args, "--vars")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_else(|| {
            let mut collected = oracle::collect_vars(&expr);
            for constraint in &constraints {
                collected.extend(oracle::collect_vars(constraint));
            }
            collected
        });
    vars.sort();
    vars.dedup();

    let binary: HashSet<String> = flag_value(&args, "--binary")
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();

    let report = crosscheck_under(&expr, &constraints, &vars, &binary, alg, target);
    println!(
        "alg={} target={target:?} constraints={} vars={vars:?}",
        alg.as_str(),
        constraints.len()
    );
    println!("dpll_count     = {}", report.dpll_count);
    println!("exhaustive     = {}", report.exhaustive_count);
    match report.bdd_count {
        Some(count) => println!("bdd_count      = {count}"),
        None => println!(
            "bdd_count      = refused({})",
            report.bdd_error.as_deref().unwrap_or("unknown")
        ),
    }
    println!("match          = {}", report.matches);
    println!("three_way      = {}", report.three_way);
    println!(
        "dpll_time      = {:.3} ms",
        report.dpll_time.as_secs_f64() * 1e3
    );
    println!(
        "exhaustive_time= {:.3} ms",
        report.exhaustive_time.as_secs_f64() * 1e3
    );
    println!(
        "bdd_time       = {:.3} ms",
        report.bdd_time.as_secs_f64() * 1e3
    );
    println!(
        "nodes={} branches={} components={} cache={}/{}",
        report.stats.nodes,
        report.stats.branches,
        report.stats.components,
        report.stats.cache_hits,
        report.stats.cache_hits + report.stats.cache_misses
    );
    if report.matches {
        ExitCode::SUCCESS
    } else {
        eprintln!("MISMATCH");
        ExitCode::from(1)
    }
}

fn run_policy(path: &str) -> ExitCode {
    let policy = match Policy::load(path) {
        Ok(policy) => policy,
        Err(error) => {
            eprintln!("策略读取失败: {error}");
            return ExitCode::from(2);
        }
    };
    let mut binary: Vec<String> = policy.binary.iter().cloned().collect();
    binary.sort();
    println!(
        "policy={} alg={} vars={:?} binary={:?}",
        path,
        policy.algebra.as_str(),
        policy.vars,
        binary
    );
    let assumption_true = policy.count_assumptions();
    println!(
        "assumptions={} assumption_true={assumption_true}",
        policy.assumptions.len()
    );

    let mut failures = 0usize;
    for (index, assertion) in policy.assertions.iter().enumerate() {
        let mut parts = Vec::new();
        let mut total = 0u128;
        let mut matches = true;
        let mut dpll_time = 0.0f64;
        let mut exhaustive_time = 0.0f64;
        let mut bdd_time = 0.0f64;
        for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
            let start = Instant::now();
            let dpll = count_models_under(
                &assertion.expr,
                &policy.assumptions,
                &policy.vars,
                &policy.binary,
                policy.algebra,
                target,
            )
            .count;
            dpll_time = dpll_time.max(start.elapsed().as_secs_f64() * 1e3);

            let start = Instant::now();
            let exhaustive = count_exhaustive_under(
                &assertion.expr,
                &policy.assumptions,
                &policy.vars,
                &policy.binary,
                policy.algebra,
                target,
            );
            exhaustive_time = exhaustive_time.max(start.elapsed().as_secs_f64() * 1e3);

            let start = Instant::now();
            let bdd = count_models_bdd(
                &assertion.expr,
                &policy.assumptions,
                &policy.vars,
                &policy.binary,
                policy.algebra,
                target,
                CROSSCHECK_BDD_LIMITS,
            );
            bdd_time = bdd_time.max(start.elapsed().as_secs_f64() * 1e3);

            matches &= dpll == exhaustive;
            let label = match target {
                Ternary::False => "false",
                Ternary::True => "true",
                Ternary::Unknown => "unknown",
            };
            match bdd {
                Ok(count) => {
                    matches &= count == dpll;
                    parts.push(format!("{label}={dpll}/{exhaustive}/{count}"));
                }
                Err(error) => {
                    parts.push(format!("{label}={dpll}/{exhaustive}/refused({error})"));
                }
            }
            total += dpll;
        }
        matches &= total == assumption_true;
        if !matches {
            failures += 1;
        }
        println!(
            "[{index}] {name} {parts} total={total} match={matches} dpll={dpll_time:.3}ms exh={exhaustive_time:.3}ms bdd={bdd_time:.3}ms",
            index = index + 1,
            name = assertion.name,
            parts = parts.join(" "),
        );
    }

    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        eprintln!("MISMATCH: {failures} 条断言与穷举/BDD 不一致");
        ExitCode::from(1)
    }
}

fn run_random(cases: usize) -> ExitCode {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut failures = 0usize;
    let mut total = 0usize;
    let mut refused = 0usize;
    for n in 1..=12usize {
        let vars: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
        let mut max_dpll = 0.0f64;
        let mut max_exhaustive = 0.0f64;
        let mut max_bdd = 0.0f64;
        let mut nodes = 0u64;
        let mut branches = 0u64;
        let mut components = 0u64;
        let mut hits = 0u64;
        let mut lookups = 0u64;
        for _ in 0..cases {
            for alg in Algebra::ALL {
                for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
                    let expr = random_expr(&mut rng, &vars, 4);
                    let report = crosscheck(&expr, &vars, &HashSet::new(), alg, target);
                    total += 1;
                    if report.bdd_count.is_none() {
                        refused += 1;
                    }
                    if !report.matches {
                        failures += 1;
                        eprintln!(
                            "MISMATCH n={n} alg={} target={target:?} expr={expr:?} dpll={} exhaustive={} bdd={:?}({})",
                            alg.as_str(),
                            report.dpll_count,
                            report.exhaustive_count,
                            report.bdd_count,
                            report.bdd_error.as_deref().unwrap_or("ok"),
                        );
                    }
                    max_dpll = max_dpll.max(report.dpll_time.as_secs_f64() * 1e3);
                    max_exhaustive = max_exhaustive.max(report.exhaustive_time.as_secs_f64() * 1e3);
                    max_bdd = max_bdd.max(report.bdd_time.as_secs_f64() * 1e3);
                    nodes += report.stats.nodes;
                    branches += report.stats.branches;
                    components += report.stats.components;
                    hits += report.stats.cache_hits;
                    lookups += report.stats.cache_hits + report.stats.cache_misses;
                }
            }
        }
        println!(
            "n={n:2} cases={cases:2} max_dpll={max_dpll:8.3}ms max_exh={max_exhaustive:9.3}ms max_bdd={max_bdd:8.3}ms nodes={nodes} branches={branches} comps={components} cache={hits}/{lookups}",
        );
    }
    println!("total={total} failures={failures} bdd_refused={refused}");
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// DPLL vs BDD 对拍（不跑穷举），用于 n>12 的第三方校验。
fn run_big(cases: usize, max_n: usize) -> ExitCode {
    let mut rng = Rng(0x5EED_1234_ABCD_EF01);
    let mut failures = 0usize;
    let mut total = 0usize;
    let mut refused = 0usize;
    for n in (13..=max_n).step_by(4) {
        let vars: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
        let mut max_dpll = 0.0f64;
        let mut max_bdd = 0.0f64;
        for _ in 0..cases {
            for alg in Algebra::ALL {
                for target in [Ternary::False, Ternary::True, Ternary::Unknown] {
                    let expr = random_expr(&mut rng, &vars, 4);
                    let start = Instant::now();
                    let dpll =
                        count_models_under(&expr, &[], &vars, &HashSet::new(), alg, target).count;
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
                            if count != dpll {
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
    println!("big_total={total} failures={failures} bdd_refused={refused}");
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
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

fn parse_target(value: &str) -> Result<Ternary, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "t" | "1" => Ok(Ternary::True),
        "false" | "f" | "0" => Ok(Ternary::False),
        "unknown" | "x" | "?" => Ok(Ternary::Unknown),
        other => Err(format!("未知目标值 {other:?}；用 true/false/unknown")),
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1).cloned())
}

fn flag_values(args: &[String], name: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if args[index] == name
            && let Some(value) = args.get(index + 1)
            && !value.starts_with("--")
        {
            values.push(value.clone());
            index += 2;
            continue;
        }
        index += 1;
    }
    values
}

fn usage() {
    println!(
        "trit-count — 三值表达式精确模型计数（dual-rail CNF + DPLL + 组件缓存）\n\
         \n\
         用法:\n\
           trit-count --expr \"a & b\" [--constraint \"a\"] [--constraint \"!c\"] [--vars a,b,c] [--binary a] [--alg k3|l3|b3] [--target true|false|unknown]\n\
           trit-count --random [--cases N]\n\
           trit-count --big [MAX_N] [--cases N]\n\
           trit-count --policy examples/policy.json\n\
         \n\
         说明:\n\
           --policy 读取策略 JSON，展开 definitions，把 assumptions 当约束，逐条断言统计\n\
           true/false/unknown 的赋值数，并与穷举、BDD 三方对拍（总数必须等于前提为真的赋值数）。\n\
           --constraint 可重复；每条约束必须为 true，计数只在满足全部约束的赋值上进行。\n\
           --vars 省略时从表达式与约束收集；显式列出未使用变量会按其域计数。\n\
           --binary 中的变量只取 false/true；其余变量取 false/true/unknown。\n\
           --random 对 n=1..12 的随机表达式做 DPLL vs 穷举 vs BDD 三方对拍。\n\
           --big 对 n=13..MAX_N 的随机表达式做 DPLL vs BDD 两方对拍（穷举在这个规模跑不动）；\n\
           BDD 侧受 200k 节点预算限制，超预算只计 refused，不算不一致。"
    );
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
