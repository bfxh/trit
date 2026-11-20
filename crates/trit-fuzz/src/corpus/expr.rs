//! `random_expr`（拆出 `corpus.rs` 守模块行数；行为不变，空变量表仍按契约 panic）。

use trit_bench::SeedRng;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

/// 一棵深度不超过 `depth` 的随机三值表达式，叶子只取 `vars` 里的变量或常量。
///
/// 分支算子：`Not / and / or / xor / implies / equiv`（六选一，`Not` 只在一元位）。
/// `depth == 0` 或每层 1/4 概率提前落到叶子——这让小树常见，大树不至于把测试拖死。
#[must_use]
pub fn random_expr(rng: &mut SeedRng, vars: &[String], depth: usize) -> Expr {
    assert!(!vars.is_empty(), "变量表为空就没法生成叶子");
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
    let kid = |rng: &mut SeedRng| random_expr(rng, vars, depth - 1);
    match rng.below(6) {
        0 => Expr::Not(Box::new(kid(rng))),
        1 => Expr::and(kid(rng), kid(rng)),
        2 => Expr::or(kid(rng), kid(rng)),
        3 => Expr::xor(kid(rng), kid(rng)),
        4 => Expr::implies(kid(rng), kid(rng)),
        _ => Expr::equiv(kid(rng), kid(rng)),
    }
}
