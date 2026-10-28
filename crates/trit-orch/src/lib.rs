#![forbid(unsafe_code)]

mod admission;
mod budget;
mod error;
mod evidence;
mod model;
mod plan;
mod selection;
mod status;

pub use admission::{admit, dependency_gate};
pub use error::Error;
pub use evidence::{EvidenceSpec, Exit, Receipt};
pub use model::{
    BlockReason, CheckReport, CheckResult, Decision, Limits, Observation, Phase, PlanPatch,
    Selection, Snapshot, TaskSpec, TokenUsage,
};
pub use plan::Plan;
pub use status::Counts;
pub use trit_core::value::Ternary;
pub use trit_schedule::{Dependency, MAX_TASKS, Resources, Task};

#[cfg(test)]
mod tests;
