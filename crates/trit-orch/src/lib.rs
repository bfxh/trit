#![forbid(unsafe_code)]

mod admission;
mod budget;
mod error;
mod model;
mod plan;
mod selection;

pub use admission::{admit, dependency_gate};
pub use error::Error;
pub use model::{
    BlockReason, CheckReport, CheckResult, Decision, Limits, Observation, Phase, Selection,
    Snapshot, TaskSpec, TokenUsage,
};
pub use plan::Plan;
pub use trit_core::value::Ternary;
pub use trit_schedule::{Dependency, MAX_TASKS, Resources, Task};

#[cfg(test)]
mod tests;
