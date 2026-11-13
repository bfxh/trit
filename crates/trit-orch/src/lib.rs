#![forbid(unsafe_code)]

mod adherence;
mod admission;
mod brief;
mod budget;
mod budget_book;
mod claim;
mod context;
mod effects;
mod error;
mod evidence;
mod evidence_rails;
mod evidence_writeback;
mod flip_rates;
mod guidance;
mod intervention;
mod lanes;
mod mass;
mod model;
mod obligation;
mod obligation_guard;
mod obligation_plan;
mod orch_metrics;
mod plan;
mod policy;
mod progress;
mod receipt;
mod selection;
mod shadow;
mod shadow_learn;
mod status;
mod tick;
mod trace;
mod trace_invariants;
mod trace_merge;
mod trace_store;

pub use adherence::{
    AdherenceReport, AdherenceViolation, GuidanceMetrics, check_adherence,
    check_adherence_with_trace,
};
pub use admission::{admit, admit_with_claim, dependency_gate};
pub use brief::{TaskBrief, render_brief, task_brief};
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
pub use effects::{
    Attempt, Effect, EffectSpec, Ledger, LedgerStore, Reconcile, Recovery, dispatch_ready,
    effect_operations, prepare_effects,
};
pub use error::Error;
pub use evidence::{EvidenceSpec, Exit, Receipt};
pub use evidence_rails::{EvidenceItem, EvidenceRails};
pub use evidence_writeback::{
    DeferredWriteback, EvidenceWritebackDeferReason, EvidenceWritebackReport,
    writeback_obligation_evidence,
};
pub use flip_rates::{FlipPrior, FlipRates, ScorePolicy};
pub use guidance::guidance_from_text;
pub use intervention::{
    Intervention, InterventionAction, InterventionPlan, InterventionPolicy, plan_interventions,
};
pub use lanes::{LaneDispute, LaneDriver, LaneReport, LaneState, resolve_lanes};
pub use mass::{UndecidedMass, undecided_mass};
pub use model::{
    BlockReason, CheckReport, CheckResult, Decision, EvidenceVerdict, Limits, Observation, Phase,
    PlanPatch, Selection, Snapshot, TaskGuidance, TaskSpec, TokenUsage,
};
pub use obligation::{
    AdmissionObligation, ObligationBatch, ObligationBudget, ObligationCost, ObligationKind,
    ObligationPolicy, ObligationSkip, ObligationSkipReason, obligations_for, schedule_obligations,
    schedule_obligations_by_score,
};
pub use obligation_guard::{
    ObligationGuard, schedule_obligations_by_score_with_guard, schedule_obligations_with_guard,
};
pub use obligation_plan::{
    DeferredObligation, ObligationPlan, ObligationPlanDeferReason, ObligationTaskLink,
    ObligationTaskPolicy, plan_patch_for_obligations, plan_patch_for_obligations_with_claims,
};
pub use orch_metrics::OrchestrationMetrics;
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
pub use shadow_learn::{
    PolicyEvaluation, PolicyLearnDeferReason, PolicyLearnDeferral, PolicyLearning,
    PolicyRecommendation, ShadowLearner, choose_policy, evaluate_policies, learn_builtin_policies,
    learn_policy,
};
pub use status::Counts;
pub use tick::{Tick, TickInput, tick};
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
