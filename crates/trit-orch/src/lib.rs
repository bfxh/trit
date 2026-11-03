#![forbid(unsafe_code)]

mod admission;
mod budget;
mod claim;
mod context;
mod effects;
mod error;
mod evidence;
mod model;
mod plan;
mod policy;
mod progress;
mod receipt;
mod selection;
mod shadow;
mod status;

pub use admission::{admit, admit_with_claim, dependency_gate};
pub use claim::{Claim, ClaimOutcome, ClaimState, ClaimStore, MAX_CLAIM_PATHS, paths_overlap};
pub use context::{
    CompactionBlock, CompactionDecision, ContextFacts, DEFAULT_THRESHOLD_BP, MAX_THRESHOLD_BP,
    MIN_THRESHOLD_BP, Obligation, Overflow, Tier, compaction_gate,
};
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
pub use receipt::{
    DeliveryReceipt, HandoffPacket, ReceiptFailure, ReceiptLedger, ReceiptState, handoff_gate,
};
pub use shadow::{ShadowReport, TraceStep, TraceStore, replay};
pub use status::Counts;
pub use trit_core::value::Ternary;
pub use trit_schedule::{Dependency, MAX_TASKS, Resources, Task};

#[cfg(test)]
mod tests;
