//! Exhaustive reference counter over the full ternary (or binary) domain.

use std::collections::{BTreeSet, HashSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;
use trit_eval::{Assignment, eval};

use crate::cnf::Cnf;

/// Counts assignments over `vars` whose evaluation equals `target`.
///
/// Binary variables range over `{False, True}`; the rest over
/// `{False, True, Unknown}`. The cost is the product of the domain sizes.
#[must_use]
pub fn count_exhaustive(
    expr: &Expr,
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> u128 {
    count_exhaustive_under(expr, &[], vars, binary, alg, target)
}

/// Counts assignments where every constraint is `True` and `expr == target`.
#[must_use]
pub fn count_exhaustive_under(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> u128 {
    let domains: Vec<&[Ternary]> = vars
        .iter()
        .map(|name| {
            if binary.contains(name) {
                &[Ternary::False, Ternary::True][..]
            } else {
                &[Ternary::False, Ternary::True, Ternary::Unknown][..]
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
        let constraints_hold = constraints
            .iter()
            .all(|constraint| eval(constraint, &assignment, alg) == Ternary::True);
        if constraints_hold && eval(expr, &assignment, alg) == target {
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

/// 对 **CNF 本体**穷举 `2^nvars` 个布尔赋值（不经过编码器）。
///
/// 用途：`--fuzz-cnf` 直接喂随机子句集时，这把尺钉的是 DPLL 自己（规范形整理、单元传播、
/// 分量分解、缓存键）。注意 `tests/cnf_fuzz.rs` 里另有一份**故意写得最朴素**的枚举，
/// 两份互为独立复核——共用一份实现就失去了"复核者不复刻被复核方 bug"的意义。
#[must_use]
pub fn count_cnf_exhaustive(cnf: &Cnf) -> u128 {
    assert!(
        cnf.nvars < 128,
        "nvars={} 超出 2^n 穷举的表示范围",
        cnf.nvars
    );
    (0..1u128 << cnf.nvars)
        .filter(|bits| cnf.clauses.iter().all(|clause| satisfied(clause, *bits)))
        .count() as u128
}

fn satisfied(clause: &[i32], bits: u128) -> bool {
    clause.iter().any(|&lit| {
        let idx = lit.unsigned_abs() as usize - 1;
        let value = (bits >> idx) & 1 == 1;
        if lit > 0 { value } else { !value }
    })
}

/// Collects variable names in sorted order.
#[must_use]
pub fn collect_vars(expr: &Expr) -> Vec<String> {
    let mut set = BTreeSet::new();
    fn walk(expr: &Expr, set: &mut BTreeSet<String>) {
        match expr {
            Expr::Const(_) => {}
            Expr::Var(name) => {
                set.insert(name.clone());
            }
            Expr::Not(a) => walk(a, set),
            Expr::And(a, b)
            | Expr::Or(a, b)
            | Expr::Xor(a, b)
            | Expr::Implies(a, b)
            | Expr::Equiv(a, b) => {
                walk(a, set);
                walk(b, set);
            }
        }
    }
    walk(expr, &mut set);
    set.into_iter().collect()
}
