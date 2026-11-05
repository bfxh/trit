//! 三值测试语料生成（确定性）。
//!
//! 立项判据不是"感觉重复"，是一条能量化的尺：
//! `grep -rl "<< 13" --include=*.rs crates/ | xargs grep -l ">> 7\|>> 17"` 数到 **64 个文件**
//! 各自手写一条 xorshift（移位组合略有出入），其中 `trit-count` 一个包里就有两份**逐字相同**的
//! `random_expr`（`src/main.rs` 与 `tests/crosscheck.rs`，本轮已并到这里）。
//! RNG 本体复用 [`trit_bench::SeedRng`]（同一组 13/7/17 移位），所以"换包"不改变序列口径。
//!
//! 只做语料，不做求解：这里出现的任何表达式都不该被当成"产品输入"。

use trit_bench::SeedRng;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

/// 长度为 `len` 的三值向量，取值 `{False, True, Unknown}`。
///
/// 口径：每个位置独立取 `rng.below(3)`，与 [`SeedRng::trits`] 的 `-1/0/+1` 同一把尺。
#[must_use]
pub fn random_trits(rng: &mut SeedRng, len: usize) -> Vec<Ternary> {
    (0..len)
        .map(|_| match rng.below(3) {
            0 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        })
        .collect()
}

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

/// `n` 个变量名（`a`、`b`、`c`…；超过 26 个时接 `a27`…）。
#[must_use]
pub fn var_names(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            if i < 26 {
                String::from((b'a' + i as u8) as char)
            } else {
                format!("v{i}")
            }
        })
        .collect()
}
