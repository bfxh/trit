use std::collections::BTreeMap;
use std::fmt;

use crate::chaos::{MutationPlan, Mutator};
use crate::findings::ScanReport;
use crate::rule::{StructuralRule, run_rules};
use crate::snapshot::WorldSnapshot;
use trit_core::Ternary;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CheckpointId(pub u64);

#[derive(Clone, Debug)]
struct Checkpoint {
    snapshot: WorldSnapshot,
    hash: u64,
    entity_count: usize,
    resource_count: usize,
    collider_count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryCheck {
    pub name: &'static str,
    pub trit: Ternary,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub checks: Vec<RecoveryCheck>,
}

impl VerifyReport {
    pub fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.trit.is_true())
    }

    pub fn failures(&self) -> impl Iterator<Item = &RecoveryCheck> {
        self.checks.iter().filter(|check| !check.trit.is_true())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryError {
    Store(String),
    Restore(String),
    Verify(String),
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(message) => write!(f, "checkpoint store error: {}", message),
            Self::Restore(message) => write!(f, "restore error: {}", message),
            Self::Verify(message) => write!(f, "verification error: {}", message),
        }
    }
}

impl std::error::Error for RecoveryError {}

pub trait CheckpointStore {
    fn capture(&mut self, snapshot: &WorldSnapshot) -> Result<CheckpointId, RecoveryError>;

    fn restore(&mut self, id: CheckpointId) -> Result<WorldSnapshot, RecoveryError>;

    fn verify(
        &mut self,
        id: CheckpointId,
        snapshot: &WorldSnapshot,
    ) -> Result<VerifyReport, RecoveryError>;
}

#[derive(Default)]
pub struct InMemoryCheckpointStore {
    checkpoints: BTreeMap<u64, Checkpoint>,
    next_id: u64,
    fail_next_restore: bool,
    fail_all_restores: bool,
    corrupt_next_verify: bool,
}

impl InMemoryCheckpointStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fail_next_restore(&mut self) {
        self.fail_next_restore = true;
    }

    pub fn fail_all_restores(&mut self) {
        self.fail_all_restores = true;
    }

    pub fn corrupt_next_verify(&mut self) {
        self.corrupt_next_verify = true;
    }

    pub fn len(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.checkpoints.is_empty()
    }
}

impl CheckpointStore for InMemoryCheckpointStore {
    fn capture(&mut self, snapshot: &WorldSnapshot) -> Result<CheckpointId, RecoveryError> {
        let snapshot = snapshot.clone().canonicalized();
        let hash = snapshot.stable_hash();
        self.next_id += 1;
        let id = CheckpointId(self.next_id);
        self.checkpoints.insert(
            id.0,
            Checkpoint {
                entity_count: snapshot.entity_count(),
                resource_count: snapshot.resource_count(),
                collider_count: snapshot.collider_count(),
                snapshot,
                hash,
            },
        );
        Ok(id)
    }

    fn restore(&mut self, id: CheckpointId) -> Result<WorldSnapshot, RecoveryError> {
        if self.fail_all_restores {
            return Err(RecoveryError::Restore(
                "all restores are disabled by chaos test".into(),
            ));
        }
        if self.fail_next_restore {
            self.fail_next_restore = false;
            return Err(RecoveryError::Restore(
                "single restore failure injected by chaos test".into(),
            ));
        }
        let checkpoint = self
            .checkpoints
            .get(&id.0)
            .ok_or_else(|| RecoveryError::Restore(format!("checkpoint {} not found", id.0)))?;
        Ok(checkpoint.snapshot.clone())
    }

    fn verify(
        &mut self,
        id: CheckpointId,
        snapshot: &WorldSnapshot,
    ) -> Result<VerifyReport, RecoveryError> {
        let checkpoint = self
            .checkpoints
            .get(&id.0)
            .ok_or_else(|| RecoveryError::Verify(format!("checkpoint {} not found", id.0)))?;
        let snapshot = snapshot.clone().canonicalized();
        let actual_hash = snapshot.stable_hash();
        let entity_count = snapshot.entity_count();
        let resource_count = snapshot.resource_count();
        let collider_count = snapshot.collider_count();

        let mut checks = vec![
            RecoveryCheck {
                name: "state_hash",
                trit: Ternary::from_bool(actual_hash == checkpoint.hash),
                detail: format!("expected {}, got {}", checkpoint.hash, actual_hash),
            },
            RecoveryCheck {
                name: "entity_count",
                trit: Ternary::from_bool(entity_count == checkpoint.entity_count),
                detail: format!("expected {}, got {}", checkpoint.entity_count, entity_count),
            },
            RecoveryCheck {
                name: "resource_count",
                trit: Ternary::from_bool(resource_count == checkpoint.resource_count),
                detail: format!(
                    "expected {}, got {}",
                    checkpoint.resource_count, resource_count
                ),
            },
            RecoveryCheck {
                name: "collider_count",
                trit: Ternary::from_bool(collider_count == checkpoint.collider_count),
                detail: format!(
                    "expected {}, got {}",
                    checkpoint.collider_count, collider_count
                ),
            },
            RecoveryCheck {
                name: "tick",
                trit: Ternary::from_bool(snapshot.tick == checkpoint.snapshot.tick),
                detail: format!(
                    "expected {}, got {}",
                    checkpoint.snapshot.tick, snapshot.tick
                ),
            },
        ];

        let inject_verify_failure = self.corrupt_next_verify;
        self.corrupt_next_verify = false;
        if inject_verify_failure {
            checks.push(RecoveryCheck {
                name: "injected_verify_failure",
                trit: Ternary::False,
                detail: "verification failure injected by chaos test".into(),
            });
        }

        Ok(VerifyReport { checks })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreLevel {
    Checkpoint,
    Pristine,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecoveryStatus {
    Restored {
        level: RestoreLevel,
        verify: VerifyReport,
    },
    Quarantined {
        reason: String,
        verify: Option<VerifyReport>,
    },
}

#[derive(Clone, Debug)]
pub struct TransactionOutcome {
    pub report: ScanReport,
    pub recovery: RecoveryStatus,
    pub apply_error: Option<String>,
}

pub struct RecoveryCoordinator<S> {
    store: S,
    rules: Vec<Box<dyn StructuralRule>>,
    pristine: Option<CheckpointId>,
}

impl<S: CheckpointStore> RecoveryCoordinator<S> {
    pub fn new(store: S, rules: Vec<Box<dyn StructuralRule>>) -> Self {
        Self {
            store,
            rules,
            pristine: None,
        }
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut S {
        &mut self.store
    }

    pub fn capture_pristine(
        &mut self,
        target: &impl Mutator,
    ) -> Result<CheckpointId, RecoveryError> {
        let id = self.store.capture(&target.snapshot())?;
        self.pristine = Some(id);
        Ok(id)
    }

    pub fn run(
        &mut self,
        target: &mut impl Mutator,
        plan: &MutationPlan,
    ) -> Result<TransactionOutcome, RecoveryError> {
        let baseline = target.snapshot().canonicalized();
        let baseline_report = run_rules(&self.rules, &baseline);
        let checkpoint = self.store.capture(&baseline)?;

        let apply_error = match plan.apply_to(target) {
            Ok(_) => None,
            Err(error) => Some(error.to_string()),
        };

        let report = run_rules(&self.rules, &target.snapshot());

        let (mut last_reason, mut last_verify) =
            match self.try_restore(target, checkpoint, &baseline_report) {
                Ok(verify) => {
                    return Ok(TransactionOutcome {
                        report,
                        recovery: RecoveryStatus::Restored {
                            level: RestoreLevel::Checkpoint,
                            verify,
                        },
                        apply_error,
                    });
                }
                Err((reason, verify)) => (reason, verify),
            };

        let pristine = self.pristine;
        if let Some(pristine) = pristine
            && pristine != checkpoint
        {
            match self.try_restore(target, pristine, &baseline_report) {
                Ok(verify) => {
                    return Ok(TransactionOutcome {
                        report,
                        recovery: RecoveryStatus::Restored {
                            level: RestoreLevel::Pristine,
                            verify,
                        },
                        apply_error,
                    });
                }
                Err((reason, verify)) => {
                    last_reason = format!("{}; pristine fallback: {}", last_reason, reason);
                    if verify.is_some() {
                        last_verify = verify;
                    }
                }
            }
        }

        Ok(TransactionOutcome {
            report,
            recovery: RecoveryStatus::Quarantined {
                reason: last_reason,
                verify: last_verify,
            },
            apply_error,
        })
    }

    fn try_restore(
        &mut self,
        target: &mut impl Mutator,
        checkpoint: CheckpointId,
        baseline_report: &ScanReport,
    ) -> Result<VerifyReport, (String, Option<VerifyReport>)> {
        let snapshot = self
            .store
            .restore(checkpoint)
            .map_err(|error| (error.to_string(), None))?;
        target.load_snapshot(snapshot);

        let verify = self
            .store
            .verify(checkpoint, &target.snapshot())
            .map_err(|error| (error.to_string(), None))?;
        if !verify.passed() {
            return Err(("restored state failed verification".into(), Some(verify)));
        }

        let restored_report = run_rules(&self.rules, &target.snapshot());
        if restored_report.signature() != baseline_report.signature() {
            return Err((
                "post-recovery scan differs from baseline".into(),
                Some(verify),
            ));
        }

        Ok(verify)
    }
}
