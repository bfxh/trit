//! Exhaustive reference counter over the full ternary (or binary) domain.

use std::collections::{BTreeSet, HashSet};

use trit_core::algebra::Algebra;
use trit_core::ast::Expr;
use trit_core::value::Ternary;
use trit_eval::{Assignment, eval};

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
        if eval(expr, &assignment, alg) == target {
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
