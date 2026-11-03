//! Exact model counting for ternary expressions.
//!
//! Pipeline: expression → dual-rail CNF (`cnf`) → DPLL + component caching
//! (`dpll`), with an exhaustive oracle (`oracle`) for cross-checking at
//! small variable counts.
//!
//! Counts are `u128`; a subproblem with 128 or more free Boolean variables
//! panics instead of wrapping.

#![forbid(unsafe_code)]

pub mod cnf;
pub mod dpll;
pub mod oracle;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;

pub use dpll::{CountResult, CountStats};

/// Counts satisfying assignments through the CNF + DPLL path.
#[must_use]
pub fn count_models(
    expr: &Expr,
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> CountResult {
    let cnf = cnf::Encoder::new(alg, vars, binary).encode(expr, target);
    dpll::count_cnf(&cnf)
}

/// One cross-check between the DPLL counter and the exhaustive oracle.
#[derive(Clone, Debug)]
pub struct CrosscheckReport {
    /// Count from the CNF + DPLL engine.
    pub dpll_count: u128,
    /// Count from exhaustive enumeration.
    pub exhaustive_count: u128,
    /// Whether both counts agree.
    pub matches: bool,
    /// DPLL counters.
    pub stats: CountStats,
    /// DPLL wall time.
    pub dpll_time: Duration,
    /// Exhaustive wall time.
    pub exhaustive_time: Duration,
}

/// Runs both counters and compares them.
#[must_use]
pub fn crosscheck(
    expr: &Expr,
    vars: &[String],
    binary: &HashSet<String>,
    alg: Algebra,
    target: Ternary,
) -> CrosscheckReport {
    let start = Instant::now();
    let result = count_models(expr, vars, binary, alg, target);
    let dpll_time = start.elapsed();

    let start = Instant::now();
    let exhaustive_count = oracle::count_exhaustive(expr, vars, binary, alg, target);
    let exhaustive_time = start.elapsed();

    CrosscheckReport {
        dpll_count: result.count,
        exhaustive_count,
        matches: result.count == exhaustive_count,
        stats: result.stats,
        dpll_time,
        exhaustive_time,
    }
}
