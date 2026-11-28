use crate::Resources;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("task count {count} exceeds limit {max}")]
    TooManyTasks { count: usize, max: usize },
    #[error("blank task id: {id:?}")]
    BlankTaskId { id: String },
    #[error("duplicate task id: {id}")]
    DuplicateTask { id: String },
    #[error("task {task} repeats predecessor {dependency}")]
    DuplicateDependency { task: String, dependency: String },
    #[error("task {task} references unknown predecessor {dependency}")]
    UnknownDependency { task: String, dependency: String },
    #[error("dependency cycle leaves tasks blocked: {blocked:?}")]
    Cycle { blocked: Vec<String> },
    #[error("snapshot revision {actual} does not match plan revision {expected}")]
    RevisionMismatch { expected: u64, actual: u64 },
    #[error("snapshot task ids differ: missing {missing:?}, unexpected {unexpected:?}")]
    ObservationMismatch {
        missing: Vec<String>,
        unexpected: Vec<String>,
    },
    #[error("workers must be at least 1, got 0")]
    NoWorkers,
    #[error("task {task} used {used} tokens, exceeding its limit {max_tokens}")]
    TaskTokenLimitExceeded {
        task: String,
        used: u64,
        max_tokens: u64,
    },
    #[error("snapshot occupies {occupied} workers, exceeding limit {workers}")]
    WorkerLimitExceeded { occupied: usize, workers: usize },
    #[error("snapshot reserves resources {reserved:?}, exceeding capacity {capacity:?}")]
    ResourceLimitExceeded {
        reserved: Resources,
        capacity: Resources,
    },
    #[error("observed {observed} plus reserved {reserved} tokens exceeds limit {limit}")]
    TokenLimitExceeded {
        observed: u64,
        reserved: u64,
        limit: u64,
    },
    #[error("token addition overflow for task {task:?}: {left} + {right}")]
    TokenOverflow {
        task: Option<String>,
        left: u64,
        right: u64,
    },
    #[error("task {task} overflows resources: {reserved:?} + {additional:?}")]
    ResourceOverflow {
        task: String,
        reserved: Resources,
        additional: Resources,
    },
    #[error("task {task} overflows worker count: {occupied} + 1")]
    WorkerOverflow { task: String, occupied: usize },
    #[error("patch base revision {base} is stale, plan is at {current}")]
    StalePatch { base: u64, current: u64 },
    #[error("patch touches task {id} more than once")]
    PatchDuplicate { id: String },
    #[error("patch adds task {id} that already exists")]
    PatchAddExists { id: String },
    #[error("patch targets task {id} that is not in the plan")]
    PatchTargetMissing { id: String },
    #[error("task {id} is in flight; it cannot be removed or redefined")]
    ProtectedInFlight { id: String },
    #[error("task {id} epoch overflows u64")]
    EpochOverflow { id: String },
    #[error("plan revision overflows u64")]
    RevisionOverflow,
    #[error("effect {operation} is in doubt; reconcile before retrying")]
    EffectInDoubt { operation: String },
    #[error("effect {operation} already happened")]
    EffectAlreadyApplied { operation: String },
    #[error("stale fence {given} for {operation}, current is {current}")]
    StaleFence {
        operation: String,
        given: u64,
        current: u64,
    },
    #[error("attempt {given} of {operation} does not match current attempt {current}")]
    AttemptMismatch {
        operation: String,
        given: u64,
        current: u64,
    },
    #[error("unknown effect operation {operation}")]
    UnknownOperation { operation: String },
    #[error(
        "cannot report completion: pending {pending}, in flight {in_flight}, inconclusive {inconclusive}, conflicts {conflicts}"
    )]
    Incomplete {
        pending: usize,
        in_flight: usize,
        inconclusive: usize,
        conflicts: usize,
    },
    #[error(
        "cannot report completion: {count} settled task(s) have no trusted executor receipt: {tasks:?}"
    )]
    UnattestedEvidence { count: usize, tasks: Vec<String> },
    #[error("evidence for task {task} does not match its trusted executor receipt")]
    ReceiptMismatch { task: String },
    #[error(
        "task {task} has a self-reported executor receipt; only host-measured receipts are trusted"
    )]
    ReceiptSelfReported { task: String },
    #[error("receipt for task {task} does not carry the expected host nonce")]
    ReceiptNonceMismatch { task: String },
    #[error("receipt for task {task} is missing host-measured {field}")]
    ReceiptObservationMissing { task: String, field: &'static str },
    #[error("receipt for task {task} contradicts observation {field}")]
    ReceiptObservationMismatch { task: String, field: &'static str },
    #[error("execution command for task {task} is empty")]
    ReceiptCommandEmpty { task: String },
    #[error("execution command for task {task} is not an allow-listed test runner: {program}")]
    ReceiptCommandNotAllowed { task: String, program: String },
    #[error("execution for task {task} failed to start: {message}")]
    ReceiptExecution { task: String, message: String },
    #[error("no task {task} in the plan for the requested execution")]
    ReceiptUnknownTask { task: String },
    #[error("ledger io at {path}: {message}")]
    LedgerIo { path: String, message: String },
    #[error("ledger parse: {message}")]
    LedgerParse { message: String },
    #[error("{operation} is not ready to dispatch: {reason}")]
    NotPrepared { operation: String, reason: String },
    #[error("{operation} was prepared for epoch {prepared}, plan expects {expected}")]
    PreparedEpochMismatch {
        operation: String,
        prepared: u64,
        expected: u64,
    },
    #[error("trace io at {path}: {message}")]
    TraceIo { path: String, message: String },
    #[error("trace parse at {path}:{line}: {message}")]
    TraceParse {
        path: String,
        line: usize,
        message: String,
    },
    #[error("unsupported trace schema {found} (expected {expected})")]
    TraceSchema { found: u32, expected: u32 },
    #[error("trace ECC rejected at {path}:{line}: {reason}")]
    TraceEcc {
        path: String,
        line: usize,
        reason: String,
    },
    #[error("trace event at {path}:{line} has no payload hash")]
    TraceHashMissing { path: String, line: usize },
    #[error("trace event at {path}:{line} does not match its payload hash")]
    TraceHashMismatch { path: String, line: usize },
    #[error("trace at {path} is broken at seq {seq}")]
    TraceChainBroken { path: String, seq: u64 },
    #[error("trace at {path} has a broken chain order at chain_seq {chain_seq}")]
    TraceChainOrder { path: String, chain_seq: u64 },
    #[error("trace anchor at {path} is missing")]
    TraceAnchorMissing { path: String },
    #[error("trace anchor at {path} does not match the chain head or signature")]
    TraceAnchorMismatch { path: String },
    #[error("trace anchor at {path} is unreadable: {message}")]
    TraceAnchorParse { path: String, message: String },
    #[error("trace at {path} is truncated: watermark {watermark}, last seq {found}")]
    TraceTruncated {
        path: String,
        watermark: u64,
        found: u64,
    },
    #[error("trace watermark at {path}: {message}")]
    TraceWatermark { path: String, message: String },
    #[error("trace at {path} is too large: {size} bytes (max {max})")]
    TraceTooLarge { path: String, size: u64, max: u64 },
    #[error("trace id mismatch: expected {expected:?}, found {found:?}")]
    TraceMismatch { expected: String, found: String },
    #[error("trace seq {seq} already has a different event")]
    TraceSeqConflict { seq: u64 },
    #[error("trace is missing required input: {missing}")]
    TraceIncomplete { missing: String },
    #[error("bad input json: {message}")]
    InputParse { message: String },
    #[error("input is too large: {size} bytes (max {max})")]
    InputTooLarge { size: u64, max: u64 },
    #[error("bad context-compaction config: {message}")]
    ContextConfig { message: String },
    #[error("claim owner is blank")]
    ClaimBlankOwner,
    #[error("claim owner is too long: {owner}")]
    ClaimOwnerTooLong { owner: String },
    #[error("claim intent is blank")]
    ClaimIntentBlank,
    #[error("claim intent is too long")]
    ClaimIntentTooLong,
    #[error("claim path is blank: {path:?}")]
    ClaimBlankPath { path: String },
    #[error("claim path is too long: {path}")]
    ClaimPathTooLong { path: String },
    #[error("claim path must stay repo-relative: {path}")]
    ClaimPathEscape { path: String },
    #[error("claim has {count} paths, exceeding limit {max}")]
    ClaimTooManyPaths { count: usize, max: usize },
    #[error("claim count {count} exceeds limit {max}")]
    ClaimTooMany { count: usize, max: usize },
    #[error("claim ttl must be positive")]
    ClaimZeroTtl,
    #[error("claim expiry overflows u64")]
    ClaimTimeOverflow,
    #[error("claim fencing overflows u64")]
    ClaimFenceOverflow,
    #[error("claim id is blank")]
    ClaimIdBlank,
    #[error("duplicate claim id: {id}")]
    ClaimDuplicateId { id: String },
    #[error("claim {id} has expires_at <= acquired_at")]
    ClaimTimeRange { id: String },
    #[error("unknown claim {id}")]
    ClaimNotFound { id: String },
    #[error("claim {id} belongs to {owner}")]
    ClaimNotOwner { id: String, owner: String },
    #[error("claim {id} expired; reclaim instead of renewing")]
    ClaimExpired { id: String },
    #[error("receipt id is blank")]
    ReceiptIdBlank,
    #[error("receipt {key} already exists with id {existing}, refusing {incoming}")]
    ReceiptIdConflict {
        key: String,
        existing: String,
        incoming: String,
    },
    #[error("receipt idempotency key is blank")]
    ReceiptKeyBlank,
    #[error("unknown receipt {key}")]
    ReceiptNotFound { key: String },
    #[error("receipt rejection reason is blank")]
    ReceiptReasonBlank,
    #[error("receipt {key} already failed; it cannot advance")]
    ReceiptFailed { key: String },
    #[error("receipt {key} already acted; it cannot fail")]
    ReceiptAlreadyActed { key: String },
    #[error("receipt {key} cannot skip from {from} to {to}")]
    ReceiptSkip {
        key: String,
        from: String,
        to: String,
    },
    #[error("claim audit refused: {detail} (total={total} unverified={unverified} fabricated={fabricated})")]
    ClaimAuditRefused {
        detail: String,
        total: u32,
        unverified: u32,
        fabricated: u32,
    },
    #[error("claim audit report is unusable: {message}")]
    ClaimAuditInvalid { message: String },
    #[error("handoff field is blank: {field}")]
    HandoffFieldBlank { field: String },
    #[error("handoff packet {id} expired")]
    HandoffExpired { id: String },
    #[error("{operation}: cannot move from {from} to {to}")]
    InvalidTransition {
        operation: String,
        from: String,
        to: String,
    },
}
