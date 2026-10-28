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
        "cannot report completion: pending {pending}, in flight {in_flight}, inconclusive {inconclusive}"
    )]
    Incomplete {
        pending: usize,
        in_flight: usize,
        inconclusive: usize,
    },
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
    #[error("{operation}: cannot move from {from} to {to}")]
    InvalidTransition {
        operation: String,
        from: String,
        to: String,
    },
}

pub(crate) fn add_tokens(left: u64, right: u64, task: Option<&str>) -> Result<u64, Error> {
    left.checked_add(right).ok_or_else(|| Error::TokenOverflow {
        task: task.map(str::to_owned),
        left,
        right,
    })
}
