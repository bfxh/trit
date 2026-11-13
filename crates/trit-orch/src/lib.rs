#![forbid(unsafe_code)]

mod admission;
mod budget;
mod budget_book;
mod claim;
mod context;
mod effects;
mod error;
mod evidence;
mod flip_rates;
mod lanes;
mod mass;
mod model;
mod obligation;
mod obligation_guard;
mod plan;
mod policy;
mod progress;
mod receipt;
mod selection;
mod shadow;
mod status;
mod trace;
mod trace_invariants;
mod trace_merge;
mod trace_store;

pub use admission::{admit, admit_with_claim, dependency_gate};
pub use budget_book::{
    BudgetBook, BudgetClass, BudgetError, BudgetReport, BudgetSplit, BudgetedObligationBatch,
    DispatchBudgetError, ExecutionRequest, ExecutionReservation, ExecutionSettlement,
    dispatch_with_execution_budget, obligation_class, reserve_for_dispatch,
    schedule_obligations_with_budget, schedule_obligations_with_budget_by_score,
};
pub use claim::{
    Claim, ClaimOutcome, ClaimSet, ClaimState, ClaimStore, MAX_CLAIM_PATHS, paths_overlap,
};
pub use context::{
    CompactionBlock, CompactionDecision, ContextFacts, DEFAULT_THRESHOLD_BP, MAX_THRESHOLD_BP,
    MIN_THRESHOLD_BP, Obligation, Overflow, Tier, compaction_gate,
};
pub use effects::{Attempt, Effect, Ledger, LedgerStore, Reconcile, Recovery, dispatch_ready};
pub use error::Error;
pub use evidence::{EvidenceSpec, Exit, Receipt};
pub use flip_rates::{FlipRates, ScorePolicy};
pub use lanes::{LaneDispute, LaneDriver, LaneReport, LaneState, resolve_lanes};
pub use mass::{UndecidedMass, undecided_mass};
pub use model::{
    BlockReason, CheckReport, CheckResult, Decision, EvidenceVerdict, Limits, Observation, Phase,
    PlanPatch, Selection, Snapshot, TaskSpec, TokenUsage,
};
pub use obligation::{
    AdmissionObligation, ObligationBatch, ObligationBudget, ObligationCost, ObligationKind,
    ObligationPolicy, ObligationSkip, ObligationSkipReason, obligations_for, schedule_obligations,
    schedule_obligations_by_score,
};
pub use obligation_guard::{
    ObligationGuard, schedule_obligations_by_score_with_guard, schedule_obligations_with_guard,
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
    Actor, AppendOutcome, CausalChain, ClaimAction, ClaimRecord, CoordinationFold, EffectRecord,
    EventId, FinishCounts, HandoffRecord, Hlc, LogicalTime, ObligationOutcome, ObligationRecord,
    ReceiptAction, ReceiptRecord, RedactionPolicy, ReplayDivergence, ReplayDivergenceKind,
    ReplayReport, SpanId, TraceEvent, TraceEventKind, TraceFold, TraceId, TraceLog, TraceMetrics,
    TracePayload, TraceRecorder,
};
pub use trace_invariants::{TraceInvariantReport, TraceInvariantViolation};
pub use trace_merge::merge_logs;
pub use trace_store::{TraceEventStore, events_from_step};
pub use trit_core::tristate::TriState;
pub use trit_core::value::Ternary;
pub use trit_schedule::{Dependency, MAX_TASKS, Resources, Task};

#[cfg(test)]
mod tests;
