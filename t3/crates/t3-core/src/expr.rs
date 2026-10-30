//! Hypothesis algebra: monotone formulas over facts, with exact intervals.
//!
//! # The design constraint that makes intervals sound
//!
//! [`Expr`] has **no negation constructor**. Every hypothesis can therefore only
//! be built from `∧`, `∨` and positive fact references, which makes it monotone
//! in each fact. That single restriction buys an exact answer to the question
//! "how much can the unknowns still move this verdict?":
//!
//! ```text
//!   lower = evaluate with every Unknown fact forced to False
//!   upper = evaluate with every Unknown fact forced to True
//! ```
//!
//! For a monotone formula these are attained — they *are* the minimum and maximum
//! over all 2^k ways of resolving the unknowns — so `[lower, upper]` is a sound
//! and tight enclosure, and [`Expr::eval_interval`] computes both in one
//! traversal. A unit test confirms this against brute-force enumeration of every
//! resolution.
//!
//! If negation were allowed, the enclosure would stop being sound (flipping an
//! unknown one way could raise one branch and lower another), and the tool would
//! be back to reporting a single number it cannot justify. Negation still exists
//! in the system, but it lives at the *fact* layer: a detector decides
//! `signed.present` directly, and hypotheses consume it positively. Pushing
//! negation to the leaves is what keeps the algebra clean.
//!
//! # Reading the interval
//!
//! | `lo` | `hi` | meaning |
//! |------|------|---------|
//! | `F`  | `F`  | refuted — enough evidence was gathered to rule it out |
//! | `T`  | `T`  | established |
//! | `F`  | `T`  | undecided — and the tool knows *which* facts would settle it |
//! | `U`  | `U`  | undecided, and not even the extremes resolve it |

use crate::fact::{FactId, Facts};
use crate::Tri;

/// A monotone Boolean formula over facts.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// Reference to an observation.
    Fact(FactId),
    /// A constant, mostly useful as an identity element.
    Const(bool),
    /// Conjunction.
    And(Box<Expr>, Box<Expr>),
    /// Disjunction.
    Or(Box<Expr>, Box<Expr>),
}

/// The enclosure of a formula's value over every resolution of its unknowns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interval {
    /// Value with all unknowns resolved to false.
    pub lo: Tri,
    /// Value with all unknowns resolved to true.
    pub hi: Tri,
}

impl Interval {
    /// The enclosure of a single decided value.
    #[inline]
    #[must_use]
    pub const fn point(value: Tri) -> Self {
        Interval {
            lo: value,
            hi: value,
        }
    }

    /// True when no resolution of the unknowns can change the result.
    #[inline]
    #[must_use]
    pub const fn is_decided(&self) -> bool {
        self.lo as u8 == self.hi as u8
    }

    /// True when the formula is already refuted.
    #[inline]
    #[must_use]
    pub const fn is_refuted(&self) -> bool {
        self.hi.is_false()
    }

    /// True when the formula is established.
    #[inline]
    #[must_use]
    pub const fn is_established(&self) -> bool {
        self.lo.is_true()
    }

    /// Number of grades the interval still spans: 0 when decided, 1 when it can
    /// only move one step, 2 when it is wide open.
    #[inline]
    #[must_use]
    pub const fn spread(&self) -> u8 {
        self.hi.rank() - self.lo.rank()
    }
}

impl Expr {
    /// A hypothesis consisting of a single fact.
    #[inline]
    #[must_use]
    pub fn fact(id: FactId) -> Self {
        Expr::Fact(id)
    }

    /// A hypothesis that is always true, e.g. for unconditional notes.
    #[inline]
    #[must_use]
    pub fn always() -> Self {
        Expr::Const(true)
    }

    /// Conjunction.
    #[inline]
    #[must_use]
    pub fn and(self, other: Self) -> Self {
        Expr::And(Box::new(self), Box::new(other))
    }

    /// Disjunction.
    #[inline]
    #[must_use]
    pub fn or(self, other: Self) -> Self {
        Expr::Or(Box::new(self), Box::new(other))
    }

    /// Conjunction of many formulas; `true` when empty.
    #[must_use]
    pub fn all_of(items: Vec<Expr>) -> Self {
        let mut iter = items.into_iter();
        let Some(first) = iter.next() else {
            return Expr::Const(true);
        };
        iter.fold(first, Expr::and)
    }

    /// Disjunction of many formulas; `false` when empty.
    #[must_use]
    pub fn any_of(items: Vec<Expr>) -> Self {
        let mut iter = items.into_iter();
        let Some(first) = iter.next() else {
            return Expr::Const(false);
        };
        iter.fold(first, Expr::or)
    }

    /// Evaluates against decided facts, reporting [`Tri::Unknown`] where a fact is
    /// unknown. Equivalent to taking `hi` when the formula is monotone, but this
    /// keeps the three-valued reading explicit for callers that only want one
    /// value.
    #[must_use]
    pub fn eval(&self, facts: &Facts) -> Tri {
        match self {
            Expr::Fact(id) => facts.get(*id),
            Expr::Const(v) => Tri::from_bool(*v),
            Expr::And(a, b) => a.eval(facts).and(b.eval(facts)),
            Expr::Or(a, b) => a.eval(facts).or(b.eval(facts)),
        }
    }

    /// Computes the exact enclosure over all resolutions of the unknowns.
    ///
    /// One traversal produces both edges, so this costs the same as a single
    /// ordinary evaluation.
    #[must_use]
    pub fn eval_interval(&self, facts: &Facts) -> Interval {
        match self {
            Expr::Fact(id) => match facts.get(*id) {
                Tri::False => Interval::point(Tri::False),
                Tri::True => Interval::point(Tri::True),
                // The only place an interval is born.
                Tri::Unknown => Interval {
                    lo: Tri::False,
                    hi: Tri::True,
                },
            },
            Expr::Const(v) => Interval::point(Tri::from_bool(*v)),
            Expr::And(a, b) => {
                let (x, y) = (a.eval_interval(facts), b.eval_interval(facts));
                Interval {
                    lo: x.lo.and(y.lo),
                    hi: x.hi.and(y.hi),
                }
            }
            Expr::Or(a, b) => {
                let (x, y) = (a.eval_interval(facts), b.eval_interval(facts));
                Interval {
                    lo: x.lo.or(y.lo),
                    hi: x.hi.or(y.hi),
                }
            }
        }
    }

    /// Every fact referenced, in first-visit order, without duplicates.
    #[must_use]
    pub fn referenced(&self) -> Vec<FactId> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }

    fn collect(&self, out: &mut Vec<FactId>) {
        match self {
            Expr::Fact(id) => {
                if !out.contains(id) {
                    out.push(*id);
                }
            }
            Expr::Const(_) => {}
            Expr::And(a, b) | Expr::Or(a, b) => {
                a.collect(out);
                b.collect(out);
            }
        }
    }

    /// Number of nodes, for sanity checks in tests.
    #[must_use]
    pub fn node_count(&self) -> usize {
        match self {
            Expr::Fact(_) | Expr::Const(_) => 1,
            Expr::And(a, b) | Expr::Or(a, b) => 1 + a.node_count() + b.node_count(),
        }
    }
}

impl core::fmt::Display for Expr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Expr::Fact(id) => f.write_str(id.name()),
            Expr::Const(v) => write!(f, "{v}"),
            Expr::And(a, b) => write!(f, "({a} ∧ {b})"),
            Expr::Or(a, b) => write!(f, "({a} ∨ {b})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Expr, Interval};
    use crate::fact::{FactId, Facts};
    use crate::Tri;

    fn facts_with(pairs: &[(FactId, Tri)]) -> Facts {
        let mut f = Facts::new();
        for (id, v) in pairs {
            f.set(*id, *v, f32::NAN, "");
        }
        f
    }

    #[test]
    fn and_or_follow_kleene() {
        let expr = Expr::fact(FactId::ImportsLoader).and(Expr::fact(FactId::ImportsNetwork));
        let decided = facts_with(&[
            (FactId::ImportsLoader, Tri::True),
            (FactId::ImportsNetwork, Tri::False),
        ]);
        assert_eq!(expr.eval(&decided), Tri::False);

        let unknown = facts_with(&[
            (FactId::ImportsLoader, Tri::True),
            (FactId::ImportsNetwork, Tri::Unknown),
        ]);
        assert_eq!(expr.eval(&unknown), Tri::Unknown);
    }

    #[test]
    fn interval_is_decided_only_when_all_paths_agree() {
        let expr = Expr::fact(FactId::ImportsLoader).or(Expr::fact(FactId::ImportsNetwork));

        let refuted = facts_with(&[
            (FactId::ImportsLoader, Tri::False),
            (FactId::ImportsNetwork, Tri::False),
        ]);
        assert!(expr.eval_interval(&refuted).is_decided());

        let established = facts_with(&[
            (FactId::ImportsLoader, Tri::Unknown),
            (FactId::ImportsNetwork, Tri::True),
        ]);
        // Unknown ∨ True is True under every resolution.
        let iv = expr.eval_interval(&established);
        assert!(iv.is_established());
        assert!(iv.is_decided());

        let open = facts_with(&[
            (FactId::ImportsLoader, Tri::Unknown),
            (FactId::ImportsNetwork, Tri::False),
        ]);
        let iv = expr.eval_interval(&open);
        assert_eq!(
            iv,
            Interval {
                lo: Tri::False,
                hi: Tri::True
            }
        );
        assert_eq!(iv.spread(), 2);
    }

    /// The heart of the claim: for monotone formulas the two extreme evaluations
    /// bracket every one of the `2^k` resolutions. Checked by brute force.
    #[test]
    fn interval_equals_exhaustive_enumeration() {
        // A formula with a mix of shared and distinct facts, deliberately not a
        // simple DNF so that correlation between branches is exercised.
        let exprs = [
            Expr::fact(FactId::ImportsLoader).and(Expr::fact(FactId::ImportsNetwork)),
            Expr::fact(FactId::ImportsLoader)
                .and(Expr::fact(FactId::ImportsNetwork))
                .or(Expr::fact(FactId::ImportsInjectionTrio)),
            Expr::fact(FactId::ImportsLoader)
                .or(Expr::fact(FactId::ImportsNetwork).and(Expr::fact(FactId::ImportsCrypto)))
                .and(Expr::fact(FactId::SectionHighEntropy)),
            // Nested shared variable: (a ∧ b) ∨ (a ∧ c)
            Expr::fact(FactId::ImportsLoader)
                .and(Expr::fact(FactId::ImportsNetwork))
                .or(Expr::fact(FactId::ImportsLoader).and(Expr::fact(FactId::ImportsCrypto))),
        ];

        for expr in exprs {
            let refs = expr.referenced();
            let k = refs.len();
            assert!(k <= 8, "keep the enumeration small");

            // Start from all-unknown and enumerate every resolution.
            let mut min = Tri::True;
            let mut max = Tri::False;
            for mask in 0..(1u32 << k) {
                let mut trial = Facts::new();
                for (bit, id) in refs.iter().enumerate() {
                    let v = if mask & (1 << bit) != 0 {
                        Tri::True
                    } else {
                        Tri::False
                    };
                    trial.set(*id, v, f32::NAN, "");
                }
                let got = expr.eval(&trial);
                assert!(!got.is_unknown(), "a full resolution must be decided");
                min = min.and(got);
                max = max.or(got);
            }

            let iv = expr.eval_interval(&Facts::new());
            assert_eq!(iv.lo, min, "{expr} lower bound");
            assert_eq!(iv.hi, max, "{expr} upper bound");
        }
    }

    #[test]
    fn referencing_is_deduplicated_and_ordered() {
        let expr = Expr::fact(FactId::ImportsLoader)
            .and(Expr::fact(FactId::ImportsNetwork))
            .or(Expr::fact(FactId::ImportsLoader));
        assert_eq!(
            expr.referenced(),
            vec![FactId::ImportsLoader, FactId::ImportsNetwork]
        );
    }

    #[test]
    fn empty_folds_are_identities() {
        assert!(Expr::all_of(vec![]).eval(&Facts::new()).is_true());
        assert!(Expr::any_of(vec![]).eval(&Facts::new()).is_false());
    }

    #[test]
    fn display_is_readable() {
        let expr = Expr::fact(FactId::ImportsLoader).and(Expr::fact(FactId::ImportsNetwork));
        assert_eq!(
            expr.to_string(),
            "(imports.loader ∧ imports.network)".to_string()
        );
    }
}
