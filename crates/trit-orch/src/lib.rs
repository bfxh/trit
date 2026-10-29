#![forbid(unsafe_code)]

mod admission;
mod budget;
mod effects;
mod error;
mod evidence;
mod model;
mod plan;
mod policy;
mod progress;
mod selection;
mod shadow;
mod status;

pub use admission::{admit, dependency_gate};
pub use effects::{Attempt, Effect, Ledger, LedgerStore, Reconcile, Recovery, dispatch_ready};
pub use error::Error;
pub use evidence::{EvidenceSpec, Exit, Receipt};
pub use model::{
    BlockReason, CheckReport, CheckResult, Decision, Limits, Observation, Phase, PlanPatch,
    Selection, Snapshot, TaskSpec, TokenUsage,
};
pub use plan::Plan;
pub use policy::{
    Baseline, CriticalPath, Eager, LongestFirst, Proposal, ShadowContext, ShadowPolicy,
    bottom_levels, builtin,
};
pub use progress::{Progress, RoundReport, Summary};
pub use shadow::{ShadowReport, TraceStep, TraceStore, replay};
pub use status::Counts;
pub use trit_core::value::Ternary;
pub use trit_schedule::{Dependency, MAX_TASKS, Resources, Task};

#[cfg(test)]
mod tests;
