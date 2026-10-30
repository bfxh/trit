//! `t3-core` — the decision layer of the `t3` ternary triage engine.
//!
//! This crate contains no I/O, no parsing and no unsafe code. It exists to answer
//! one question as rigorously as possible: *given a bag of observations, some of
//! which are missing, what is the strongest statement I am entitled to make?*
//!
//! # The three layers
//!
//! ```text
//!   Tri        K3, strong Kleene. Values F/U/T; ∧ = min, ∨ = max.
//!    ↑         Used wherever a single question has a single answer that may be
//!    |         withheld.
//!   B4         Belnap FOUR. Two independent rails; adds "both" (conflict) to the
//!    ↑         three values above. Used to fuse detectors whose disagreement is
//!    |         itself the finding.
//!   Expr       Monotone formulas over facts, negated only at the leaves.
//!    ↑         Enables exact interval evaluation: [lo, hi] brackets every
//!    |         resolution of the unknowns, so "I don't know" comes with a width.
//!   Verdict    Grades over hypotheses joined by lattice max, plus the
//!              counterfactual list of facts that would settle the question.
//! ```
//!
//! # Why not a score
//!
//! A single number invites a threshold, and a threshold hides the difference
//! between "measured and clean" and "not measured". Every mechanism here is chosen
//! to keep that difference visible: facts default to `Unknown`, bands report
//! `Unknown` inside themselves, formulas carry their unknowns into the interval,
//! and the process exit code distinguishes `3` (undecided) from `0` (clean).
//!
//! # Example
//!
//! ```
//! use t3_core::{rules, FactId, Facts, Grade};
//!
//! fn main() {
//!     // Nothing observed yet: the tool can neither refute nor confirm anything, so
//!     // the verdict is a range rather than a label.
//!     let empty = Facts::new();
//!     let verdict = rules::default_engine().verdict(&empty);
//!     assert_eq!(verdict.lo, Grade::Clean);
//!     assert_eq!(verdict.hi, Grade::Malicious);
//!     assert!(!verdict.is_decided());
//!
//!     // An established high-severity finding pins both ends. Note that it takes a
//!     // *finding*: capability alone (the injection APIs) only reaches Suspicious,
//!     // because debuggers and browsers link those by design.
//!     let mut facts = Facts::new();
//!     facts.set_bool(FactId::SigHigh, true, "known tool fingerprint");
//!     let verdict = rules::default_engine().verdict(&facts);
//!     assert_eq!(verdict.lo, Grade::Malicious);
//!     assert_eq!(verdict.hi, Grade::Malicious);
//!     assert!(verdict.is_decided());
//! }
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic)]
#![allow(clippy::must_use_candidate)]

pub mod b4;
pub mod band;
pub mod expr;
pub mod fact;
pub mod rules;
pub mod tri;
pub mod verdict;

pub use b4::B4;
pub use band::{quantile_of, Band};
pub use expr::{Expr, Interval};
pub use fact::{Cell, FactId, Facts};
pub use tri::Tri;
pub use verdict::{Engine, Format, Grade, Hypothesis, Judgment, Verdict};
