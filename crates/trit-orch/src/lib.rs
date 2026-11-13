#![forbid(unsafe_code)]

mod admission;
mod budget;
mod claim;
mod context;
mod effects;
mod error;
mod evidence;
mod model;
mod obligation;
mod plan;
mod policy;
mod progress;
mod receipt;
mod selection;
mod shadow;
mod status;
mod trace;
mod trace_store;

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
    BlockReason, CheckReport, CheckResult, Decision, EvidenceVerdict, Limits, Observation, Phase,
    PlanPatch, Selection, Snapshot, TaskSpec, TokenUsage,
};
pub use obligation::{
    AdmissionObligation, ObligationBatch, ObligationBudget, ObligationCost, ObligationKind,
    ObligationPolicy, ObligationSkip, ObligationSkipReason, obligations_for, schedule_obligations,
};
pub use plan::Plan;
pub use policy::{
    Baseline, CriticalPath, Eager, LongestFirst, Proposal, ShadowContext, ShadowPolicy,
    bottom_levels, builtin,
};
pub use progress::{Progress, RoundReport, Summary};
pub use receipt::{
    DeliveryReceipt, HandoffPacket, ReceiptFailure, ReceiptLedger, ReceiptState, handoff_gate,
    handoff_obligation, receipt_obligation,
};
pub use shadow::{ShadowReport, TraceStep, TraceStore, replay};
pub use status::Counts;
pub use trace::{
    Actor, AppendOutcome, EventId, LogicalTime, SpanId, TraceEvent, TraceEventKind, TraceFold,
    TraceId, TraceLog, TracePayload,
};
pub use trace_store::{TraceEventStore, events_from_step};
pub use trit_core::value::Ternary;
pub use trit_schedule::{Dependency, MAX_TASKS, Resources, Task};

#[cfg(test)]
mod tests;
