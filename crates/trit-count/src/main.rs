use std::collections::HashSet;
use std::process::ExitCode;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_count::{crosscheck, oracle};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args.iter().any(|arg| arg == "--help" || arg == "-h") {
        usage();
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|arg| arg == "--random") {
        let cases = flag_value(&args, "--cases")
            .and_then(|value| value.parse().ok())
            .unwrap_or(2);
        return run_random(cases);
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
        .unwrap_or_else(|| oracle::collect_vars(&expr));
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

    let report = crosscheck(&expr, &vars, &binary, alg, target);
    println!("alg={} target={target:?} vars={vars:?}", alg.as_str());
    println!("dpll_count     = {}", report.dpll_count);
    println!("exhaustive     = {}", report.exhaustive_count);
    println!("match          = {}", report.matches);
    println!(
        "dpll_time      = {:.3} ms",
        report.dpll_time.as_secs_f64() * 1e3
    );
    println!(
        "exhaustive_time= {:.3} ms",
        report.exhaustive_time.as_secs_f64() * 1e3
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

fn run_random(cases: usize) -> ExitCode {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut failures = 0usize;
    let mut total = 0usize;
    for n in 1..=12usize {
        let vars: Vec<String> = (0..n).map(|i| format!("v{i}")).collect();
        let mut max_dpll = 0.0f64;
        let mut max_exhaustive = 0.0f64;
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
                    if !report.matches {
                        failures += 1;
                        eprintln!(
                            "MISMATCH n={n} alg={} target={target:?} expr={expr:?} dpll={} exhaustive={}",
                            alg.as_str(),
                            report.dpll_count,
                            report.exhaustive_count
                        );
                    }
                    max_dpll = max_dpll.max(report.dpll_time.as_secs_f64() * 1e3);
                    max_exhaustive = max_exhaustive.max(report.exhaustive_time.as_secs_f64() * 1e3);
                    nodes += report.stats.nodes;
                    branches += report.stats.branches;
                    components += report.stats.components;
                    hits += report.stats.cache_hits;
                    lookups += report.stats.cache_hits + report.stats.cache_misses;
                }
            }
        }
        println!(
            "n={n:2} cases={} max_dpll={max_dpll:8.3}ms max_exh={max_exhaustive:8.3}ms nodes={nodes} branches={branches} comps={components} cache={hits}/{lookups}",
            cases
        );
    }
    println!("total={total} failures={failures}");
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

fn usage() {
    println!(
        "trit-count — 三值表达式精确模型计数（dual-rail CNF + DPLL + 组件缓存）\n\
         \n\
         用法:\n\
           trit-count --expr \"a & b\" [--vars a,b] [--binary a] [--alg k3|l3|b3] [--target true|false|unknown]\n\
           trit-count --random [--cases N]\n\
         \n\
         说明:\n\
           --vars 省略时从表达式收集；显式列出未使用变量会按其域计数。\n\
           --binary 中的变量只取 false/true；其余变量取 false/true/unknown。\n\
           --random 对 n=1..12 的随机表达式做 DPLL vs 穷举对拍。"
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
