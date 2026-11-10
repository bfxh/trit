//! 匹配族 `(x0∧y0) | (x1∧y1) | …`，但变量按 `x…x y…y` 声明：
//! 这是 BDD 教科书级的坏顺序（结宽指数），DPLL 的组件缓存本该扛得住。
//!
//! 用途是给「要不要给 DPLL 加 watched literals」定靶子：打印每档的
//! 节点数、耗时与 **µs/节点**（µs/节点越大说明单节点开销越不在传播上）。
//! BDD 一侧只跑到 `CROSSCHECK_BDD_LIMITS`，超预算报 refused 而不是把机器吃满。

use std::collections::HashSet;
use std::time::Instant;

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::parser::Parser;
use trit_core::value::Ternary;
use trit_count::{CROSSCHECK_BDD_LIMITS, cnf, count_models_bdd, dpll};

fn family(pairs: usize) -> (Expr, Vec<String>, HashSet<String>) {
    let xs: Vec<String> = (0..pairs).map(|index| format!("x{index}")).collect();
    let ys: Vec<String> = (0..pairs).map(|index| format!("y{index}")).collect();
    // 表达式里 x 与 y 交错出现，声明顺序却是「所有 x 再所有 y」。
    let mut source = String::new();
    for (x, y) in xs.iter().zip(&ys) {
        let term = format!("({x} & {y})");
        source = if source.is_empty() {
            term
        } else {
            format!("({source} | {term})")
        };
    }
    let declared: Vec<String> = xs.iter().chain(ys.iter()).cloned().collect();
    let binary: HashSet<String> = declared.iter().cloned().collect();
    (
        Parser::parse(&source).expect("matching family parses"),
        declared,
        binary,
    )
}

fn main() {
    println!("pairs rails clauses dpll_ms nodes branches comps cache us/node bdd");
    for pairs in [8usize, 12, 16, 20, 24, 28, 32, 40] {
        let (expr, declared, binary) = family(pairs);
        let encoded =
            cnf::Encoder::new(Algebra::Kleene, &declared, &binary).encode(&expr, Ternary::True);
        let start = Instant::now();
        let result = dpll::count_cnf(&encoded);
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let per_node = if result.stats.nodes == 0 {
            0.0
        } else {
            ms * 1_000.0 / result.stats.nodes as f64
        };
        let lookups = result.stats.cache_hits + result.stats.cache_misses;
        let bdd = match count_models_bdd(
            &expr,
            &[],
            &declared,
            &binary,
            Algebra::Kleene,
            Ternary::True,
            CROSSCHECK_BDD_LIMITS,
        ) {
            Ok(count) if count == result.count => "ok".to_string(),
            Ok(count) => format!("MISMATCH {count}"),
            Err(error) => format!("refused({error})"),
        };
        println!(
            "{pairs:>5} {:>5} {:>7} {:>7.1} {:>5} {:>8} {:>5} {:>4}/{:<4} {:>7.1} {bdd}",
            encoded.nvars,
            encoded.clauses.len(),
            ms,
            result.stats.nodes,
            result.stats.branches,
            result.stats.components,
            result.stats.cache_hits,
            lookups,
            per_node,
        );
    }
}
