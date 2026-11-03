//! Exact model counting for ternary expressions.
//!
//! Pipeline: expression → dual-rail CNF (`cnf`) → DPLL + component caching
//! (`dpll`), cross-checked against an exhaustive oracle (`oracle`) at small
//! variable counts and against the dual-rail BDD counter (`trit-bdd`) at any
//! size the BDD budget allows.
//!
//! Counts are `u128`; a subproblem with 128 or more free Boolean variables
//! panics instead of wrapping.

#![forbid(unsafe_code)]

pub mod cnf;
pub mod dpll;
pub mod oracle;
pub mod policy;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use trit_bdd::BddRequest;

pub use trit_bdd::{BddError, BddLimits};
use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

pub use dpll::{CountResult, CountStats};

/// BDD 引擎的节点预算。
///
/// 随机深表达式会让 dual-rail BDD 膨胀，对拍时给一个远小于
/// `BddLimits::default()` 的上限，超了就报 `BudgetExceeded` 而不是拖死整轮。
pub const CROSSCHECK_BDD_LIMITS: BddLimits = BddLimits { max_nodes: 200_000 };

/// Counts satisfying assignments through the CNF + DPLL path.
#[must_use]
pub fn count_models(
    expr: &Expr,
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> CountResult {
    count_models_under(expr, &[], vars, binary, alg, target)
}

/// Counts assignments where every constraint is `True` and `expr == target`.
#[must_use]
pub fn count_models_under(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> CountResult {
    let cnf =
        cnf::Encoder::new(alg, vars, binary).encode_with_constraints(expr, constraints, target);
    dpll::count_cnf(&cnf)
}

/// Counts assignments through the dual-rail BDD engine (third independent path).
///
/// 返回 `Err` 表示 BDD 侧被节点预算或 `u128` 容量拦下，而不是计数不一致。
pub fn count_models_bdd(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
    limits: BddLimits,
) -> Result<u128, BddError> {
    let request = BddRequest {
        order: None,
        limits,
    };
    trit_bdd::count_models_bdd(expr, constraints, vars, binary, alg, target, &request)
}

/// Agreement between the DPLL counter, the exhaustive oracle and the BDD counter.
#[derive(Clone, Debug)]
pub struct CrosscheckReport {
    /// Count from the CNF + DPLL engine.
    pub dpll_count: u128,
    /// Count from exhaustive enumeration.
    pub exhaustive_count: u128,
    /// Count from the BDD engine; `None` when it was refused by a budget.
    pub bdd_count: Option<u128>,
    /// Why the BDD engine refused, when it did.
    pub bdd_error: Option<String>,
    /// Whether every engine that produced a count agrees.
    pub matches: bool,
    /// Whether all three engines produced a count and agree.
    pub three_way: bool,
    /// DPLL counters.
    pub stats: CountStats,
    /// DPLL wall time.
    pub dpll_time: Duration,
    /// Exhaustive wall time.
    pub exhaustive_time: Duration,
    /// BDD wall time.
    pub bdd_time: Duration,
}

/// Runs the DPLL, exhaustive and BDD counters and compares them.
#[must_use]
pub fn crosscheck(
    expr: &Expr,
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> CrosscheckReport {
    crosscheck_under(expr, &[], vars, binary, alg, target)
}

/// Runs the three counters with constraints and compares them.
#[must_use]
pub fn crosscheck_under(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> CrosscheckReport {
    crosscheck_under_with_limits(
        expr,
        constraints,
        vars,
        binary,
        alg,
        target,
        CROSSCHECK_BDD_LIMITS,
    )
}

/// 同 [`crosscheck_under`]，但 BDD 侧的节点预算由调用方给。
///
/// 预算拦下时只有两路出数：`matches` 仍可成立，`three_way` 必须为假。
#[must_use]
pub fn crosscheck_under_with_limits(
    expr: &Expr,
    constraints: &[Expr],
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
    bdd_limits: BddLimits,
) -> CrosscheckReport {
    let start = Instant::now();
    let result = count_models_under(expr, constraints, vars, binary, alg, target);
    let dpll_time = start.elapsed();

    let start = Instant::now();
    let exhaustive_count =
        oracle::count_exhaustive_under(expr, constraints, vars, binary, alg, target);
    let exhaustive_time = start.elapsed();

    let start = Instant::now();
    let bdd = count_models_bdd(expr, constraints, vars, binary, alg, target, bdd_limits);
    let bdd_time = start.elapsed();
    let (bdd_count, bdd_error) = match bdd {
        Ok(count) => (Some(count), None),
        Err(error) => (None, Some(error.to_string())),
    };

    let agrees = |count: u128| count == result.count;
    let matches = agrees(exhaustive_count) && bdd_count.is_none_or(agrees);
    let three_way = bdd_count.is_some_and(agrees);

    CrosscheckReport {
        dpll_count: result.count,
        exhaustive_count,
        bdd_count,
        bdd_error,
        matches,
        three_way,
        stats: result.stats,
        dpll_time,
        exhaustive_time,
        bdd_time,
    }
}
