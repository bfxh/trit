//! 轨迹不变量：在 CI/回放中检查因果、顺序与 fence。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    Effect, EvidenceVerdict, ObligationKind, ReceiptAction, ReceiptState, Reconcile, Ternary,
    TraceEventKind, TraceLog, TracePayload,
};

/// 一条轨迹不变量违例。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TraceInvariantViolation {
    MissingSeq {
        from: u64,
        to: u64,
    },
    MissingCausation {
        event: String,
        cause: String,
    },
    EffectOutOfOrder {
        operation: String,
        seq: u64,
        detail: String,
    },
    StaleFence {
        subject: String,
        seq: u64,
        given: u64,
        current: u64,
    },
    ReceiptOutOfOrder {
        key: String,
        seq: u64,
        detail: String,
    },
    HandoffWithoutObligation {
        event: String,
    },
    UnknownWithoutObligation {
        event: String,
        subject: String,
    },
    ConflictWithoutAdjudication {
        event: String,
        subject: String,
    },
    FinishWithUndetermined {
        event: String,
        undetermined: usize,
        conflicts: usize,
        in_doubt: usize,
    },
    FinishWithoutPayload {
        event: String,
    },
    ReconcileMissing {
        operation: String,
        seq: u64,
        state: Effect,
    },
    ReconcileMismatch {
        operation: String,
        seq: u64,
        detail: String,
    },
    ReconcileWithoutPayload {
        event: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceInvariantReport {
    pub violations: Vec<TraceInvariantViolation>,
}

impl TraceInvariantReport {
    pub fn is_clean(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.violations.len()
    }
}

impl TraceLog {
    /// 检查一条轨迹的因果、状态机与 fence 不变量。
    ///
    /// 这是审计检查，不修改任何状态；违例不影响已有事件。
    pub fn check_invariants(&self) -> TraceInvariantReport {
        let mut violations = Vec::new();
        for (from, to) in self.gaps() {
            violations.push(TraceInvariantViolation::MissingSeq { from, to });
        }

        let ids: BTreeSet<&str> = self.events().map(|event| event.id.as_str()).collect();
        for event in self.events() {
            if let Some(cause) = &event.causation_id
                && !ids.contains(cause.as_str())
            {
                violations.push(TraceInvariantViolation::MissingCausation {
                    event: event.id.clone(),
                    cause: cause.clone(),
                });
            }
        }

        let mut claim_fences: BTreeMap<String, u64> = BTreeMap::new();
        let mut effect_tracks: BTreeMap<String, EffectTrack> = BTreeMap::new();
        let mut receipt_tracks: BTreeMap<String, ReceiptTrack> = BTreeMap::new();

        for event in self.events() {
            if matches!(
                event.kind,
                TraceEventKind::FinishConfirmed | TraceEventKind::FinishRefused
            ) && !matches!(event.payload.as_ref(), Some(TracePayload::Finish { .. }))
            {
                violations.push(TraceInvariantViolation::FinishWithoutPayload {
                    event: event.id.clone(),
                });
            }
            if event.kind == TraceEventKind::EffectReconciled
                && !matches!(
                    event.payload.as_ref(),
                    Some(TracePayload::EffectReconciled { .. })
                )
            {
                violations.push(TraceInvariantViolation::ReconcileWithoutPayload {
                    event: event.id.clone(),
                });
            }
            match event.payload.as_ref() {
                Some(TracePayload::Claim { claim, .. }) => {
                    if let Some(&current) = claim_fences.get(&claim.id) {
                        if claim.fencing < current {
                            violations.push(TraceInvariantViolation::StaleFence {
                                subject: claim.id.clone(),
                                seq: event.seq,
                                given: claim.fencing,
                                current,
                            });
                        }
                        claim_fences.insert(claim.id.clone(), current.max(claim.fencing));
                    } else {
                        claim_fences.insert(claim.id.clone(), claim.fencing);
                    }
                }
                Some(TracePayload::Effect { attempt, state }) => {
                    let track = effect_tracks.entry(attempt.operation.clone()).or_default();
                    if track.fence > 0 && attempt.fence < track.fence {
                        violations.push(TraceInvariantViolation::StaleFence {
                            subject: attempt.operation.clone(),
                            seq: event.seq,
                            given: attempt.fence,
                            current: track.fence,
                        });
                    }
                    if matches!(*state, Effect::Confirmed | Effect::Refuted)
                        && track.last == Some(Effect::Unknown)
                    {
                        let expected = if *state == Effect::Confirmed {
                            Reconcile::Present
                        } else {
                            Reconcile::Absent
                        };
                        if track.reconciled != Some(expected) {
                            violations.push(TraceInvariantViolation::ReconcileMissing {
                                operation: attempt.operation.clone(),
                                seq: event.seq,
                                state: *state,
                            });
                        }
                    }
                    let same_state = track.last == Some(*state);
                    if !same_state
                        && !effect_transition_allowed(
                            track.last,
                            *state,
                            track.attempt,
                            attempt.attempt,
                        )
                    {
                        violations.push(TraceInvariantViolation::EffectOutOfOrder {
                            operation: attempt.operation.clone(),
                            seq: event.seq,
                            detail: format!(
                                "state {:?} after {:?} (attempt {} -> {})",
                                state, track.last, track.attempt, attempt.attempt
                            ),
                        });
                    }
                    track.last = Some(*state);
                    track.fence = track.fence.max(attempt.fence);
                    track.attempt = attempt.attempt;
                    track.reconciled = None;
                }
                Some(TracePayload::EffectReconciled {
                    attempt,
                    observed,
                    from,
                    to,
                }) => {
                    let track = effect_tracks.entry(attempt.operation.clone()).or_default();
                    let expected_to = match *observed {
                        Reconcile::Present => Effect::Confirmed,
                        Reconcile::Absent => Effect::Refuted,
                    };
                    if track.last != Some(*from)
                        || *to != expected_to
                        || track.attempt != attempt.attempt
                    {
                        violations.push(TraceInvariantViolation::ReconcileMismatch {
                            operation: attempt.operation.clone(),
                            seq: event.seq,
                            detail: format!(
                                "from {:?} (last {:?}), observed {:?} -> {:?}, attempt {} (current {})",
                                from, track.last, observed, to, attempt.attempt, track.attempt
                            ),
                        });
                    }
                    if track.fence > 0 && attempt.fence < track.fence {
                        violations.push(TraceInvariantViolation::StaleFence {
                            subject: attempt.operation.clone(),
                            seq: event.seq,
                            given: attempt.fence,
                            current: track.fence,
                        });
                    }
                    track.reconciled = Some(*observed);
                }
                Some(TracePayload::Receipt { action, receipt }) => {
                    let track = receipt_tracks
                        .entry(receipt.idempotency_key.clone())
                        .or_default();
                    match action {
                        ReceiptAction::Recorded => {
                            if track.seen {
                                violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                    key: receipt.idempotency_key.clone(),
                                    seq: event.seq,
                                    detail: "duplicate Recorded".into(),
                                });
                            }
                            if receipt.state != ReceiptState::Sent {
                                violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                    key: receipt.idempotency_key.clone(),
                                    seq: event.seq,
                                    detail: format!("Recorded with state {:?}", receipt.state),
                                });
                            }
                            track.seen = true;
                            track.rank = receipt_rank(receipt.state);
                            track.failed = receipt.is_failed();
                        }
                        ReceiptAction::Advanced => {
                            let next = receipt_rank(receipt.state);
                            if !track.seen || track.failed {
                                violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                    key: receipt.idempotency_key.clone(),
                                    seq: event.seq,
                                    detail: "advanced without a usable Sent state".into(),
                                });
                            } else {
                                if next < track.rank {
                                    violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                        key: receipt.idempotency_key.clone(),
                                        seq: event.seq,
                                        detail: "state regression".into(),
                                    });
                                } else if next > track.rank + 1 {
                                    violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                        key: receipt.idempotency_key.clone(),
                                        seq: event.seq,
                                        detail: "state skipped".into(),
                                    });
                                }
                                if receipt.state == ReceiptState::Acted && track.rank < 2 {
                                    violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                        key: receipt.idempotency_key.clone(),
                                        seq: event.seq,
                                        detail: "Acted before Read".into(),
                                    });
                                }
                            }
                            track.seen = true;
                            track.rank = next;
                            track.failed = receipt.is_failed();
                        }
                        ReceiptAction::Rejected | ReceiptAction::Expired => {
                            if track.rank == 3 {
                                violations.push(TraceInvariantViolation::ReceiptOutOfOrder {
                                    key: receipt.idempotency_key.clone(),
                                    seq: event.seq,
                                    detail: "failure after Acted".into(),
                                });
                            }
                            track.failed = true;
                        }
                    }
                }
                Some(TracePayload::Handoff { record })
                    if record.gate != Ternary::True && record.obligation.is_none() =>
                {
                    violations.push(TraceInvariantViolation::HandoffWithoutObligation {
                        event: event.id.clone(),
                    });
                }
                Some(TracePayload::Judgment {
                    subject,
                    verdict,
                    obligations,
                }) => {
                    if *verdict == EvidenceVerdict::Neither && obligations.is_empty() {
                        violations.push(TraceInvariantViolation::UnknownWithoutObligation {
                            event: event.id.clone(),
                            subject: subject.clone(),
                        });
                    }
                    if *verdict == EvidenceVerdict::Conflicted
                        && !obligations
                            .iter()
                            .any(|obligation| obligation.kind == ObligationKind::Adjudicate)
                    {
                        violations.push(TraceInvariantViolation::ConflictWithoutAdjudication {
                            event: event.id.clone(),
                            subject: subject.clone(),
                        });
                    }
                }
                Some(TracePayload::Finish { counts })
                    if event.kind == TraceEventKind::FinishConfirmed
                        && (counts.undetermined > 0
                            || counts.conflicts > 0
                            || counts.in_doubt > 0) =>
                {
                    violations.push(TraceInvariantViolation::FinishWithUndetermined {
                        event: event.id.clone(),
                        undetermined: counts.undetermined,
                        conflicts: counts.conflicts,
                        in_doubt: counts.in_doubt,
                    });
                }
                _ => {}
            }
        }

        TraceInvariantReport { violations }
    }
}

#[derive(Default)]
struct EffectTrack {
    last: Option<Effect>,
    fence: u64,
    attempt: u64,
    reconciled: Option<Reconcile>,
}

#[derive(Default)]
struct ReceiptTrack {
    seen: bool,
    rank: u8,
    failed: bool,
}

fn receipt_rank(state: ReceiptState) -> u8 {
    match state {
        ReceiptState::Sent => 0,
        ReceiptState::Delivered => 1,
        ReceiptState::Read => 2,
        ReceiptState::Acted => 3,
    }
}

fn effect_transition_allowed(
    last: Option<Effect>,
    state: Effect,
    last_attempt: u64,
    attempt: u64,
) -> bool {
    match state {
        Effect::Prepared => {
            last.is_none() || (last == Some(Effect::Refuted) && attempt > last_attempt)
        }
        Effect::Sending => last == Some(Effect::Prepared) && attempt == last_attempt,
        Effect::Sent => last == Some(Effect::Sending) && attempt == last_attempt,
        Effect::Confirmed | Effect::Refuted => {
            matches!(last, Some(Effect::Sending | Effect::Sent | Effect::Unknown))
                && attempt == last_attempt
        }
        Effect::Unknown => {
            matches!(
                last,
                Some(Effect::Prepared | Effect::Sending | Effect::Sent)
            ) && attempt == last_attempt
        }
    }
}
