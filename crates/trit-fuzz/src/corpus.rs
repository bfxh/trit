//! 三值测试语料生成（确定性）。
//! 立项判据是精确尺（`grep -rlE "\^=.*<< 13"`），不是"感觉重复"；读数口径只算 `crates/`。
//! 只做语料，不做求解：这里出现的任何表达式都不该被当成"产品输入"。
//! RNG 本体复用 [`trit_bench::SeedRng`]（同一组 13/7/17 移位），所以"换包"不改变序列口径。

use trit_bench::SeedRng;
use trit_core::value::Ternary;

/// 生成器的元素上限：语料只喂求解器，1M 已是上限。
pub const MAX_GEN: usize = 1 << 20;

mod expr;

pub use expr::random_expr;

/// 长度为 `len` 的三值向量，取值 `{False, True, Unknown}`。
///
/// 口径：每个位置独立取 `rng.below(3)`，与 [`SeedRng::trits`] 的 `-1/0/+1` 同一把尺。
#[must_use]
pub fn random_trits(rng: &mut SeedRng, len: usize) -> Vec<Ternary> {
    assert!(len <= MAX_GEN, "长度 {len} 超过上限 {MAX_GEN}");
    (0..len)
        .map(|_| match rng.below(3) {
            0 => Ternary::False,
            1 => Ternary::True,
            _ => Ternary::Unknown,
        })
        .collect()
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
    assert!(nvars <= MAX_GEN, "nvars {nvars} 超过上限 {MAX_GEN}");
    (0..nvars).map(|_| rng.below(2) == 1).collect()
}

/// 单个三值，与 [`random_trits`] 同一口径（`0→False, 1→True, 其余→Unknown`）。
#[must_use]
pub fn trit(rng: &mut SeedRng) -> Ternary {
    match rng.below(3) {
        0 => Ternary::False,
        1 => Ternary::True,
        _ => Ternary::Unknown,
    }
}

/// 单个三值的 `-1/0/+1` 形态（位并行那边要的是数，不是 `Ternary`）。
#[must_use]
pub fn ternary_value(rng: &mut SeedRng) -> i32 {
    rng.below(3) as i32 - 1
}

/// 双极值 `±1`（二进制基线的进料：没有 0 的那一路）。
#[must_use]
pub fn bipolar(rng: &mut SeedRng) -> i32 {
    if rng.below(2) == 0 { -1 } else { 1 }
}

/// `n` 个变量名（`a`、`b`、`c`…；超过 26 个时接 `a27`…）。
#[must_use]
pub fn var_names(n: usize) -> Vec<String> {
    assert!(n <= MAX_GEN, "变量数 {n} 超过上限 {MAX_GEN}");
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
