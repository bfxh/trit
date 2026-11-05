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

/// 一条子句：按 `|lit|` 排序去重，永真（同时含 `x` 与 `¬x`）就丢掉这对。
///
/// 口径写死在这里，别散进各个 fuzz 测试：子句长度是 `clause_len` 的**上界**，
/// 小 `nvars` 时重复不可避免 ⇒ 结果可能比 `clause_len` 短，但绝不会是空子句或永真子句。
fn normalize_clause(clause: &mut Vec<i32>) {
    clause.sort_by_key(|lit| (lit.unsigned_abs(), *lit < 0));
    let mut i = 0;
    while i + 1 < clause.len() {
        if clause[i] == clause[i + 1] {
            clause.remove(i + 1);
            continue;
        }
        if clause[i].unsigned_abs() == clause[i + 1].unsigned_abs() {
            // 同一变量的两个符号都在 ⇒ 永真，两条一起撤掉，换成别的变量再试。
            clause.remove(i);
            clause.remove(i);
            continue;
        }
        i += 1;
    }
}

/// `nclauses` 条、每条不超过 `clause_len` 个文字的随机 CNF（DIMACS 口径：文字是 `±1..=nvars`）。
///
/// `clause_len == 0` 或 `nvars == 0` ⇒ 没有合法文字，返回空表（而不是悄悄造出空子句把公式判成不可满足）。
#[must_use]
pub fn random_cnf(
    rng: &mut SeedRng,
    nvars: usize,
    nclauses: usize,
    clause_len: usize,
) -> Vec<Vec<i32>> {
    if nvars == 0 || clause_len == 0 {
        return Vec::new();
    }
    (0..nclauses)
        .map(|_| {
            let mut clause: Vec<i32> = (0..clause_len)
                .map(|_| {
                    let var = (rng.below(nvars) + 1) as i32;
                    if rng.below(2) == 0 { -var } else { var }
                })
                .collect();
            normalize_clause(&mut clause);
            if clause.is_empty() {
                // 全被永真消掉了：退化成一条单文字子句，保证 CNF 的形状仍可解释。
                let var = (rng.below(nvars) + 1) as i32;
                clause.push(if rng.below(2) == 0 { -var } else { var });
            }
            clause
        })
        .collect()
}

/// `nvars` 个布尔赋值的随机一行（`true` = 变量为真）。
#[must_use]
pub fn random_assignment(rng: &mut SeedRng, nvars: usize) -> Vec<bool> {
    (0..nvars).map(|_| rng.below(2) == 1).collect()
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
