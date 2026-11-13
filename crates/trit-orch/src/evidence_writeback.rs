//! 自动证据回写：把义务生成任务的证据 rebase 到目标 epoch，写入目标双轨。
//!
//! 生成任务必须先 Settled，且证据属于生成任务当前 epoch；回写按 source 幂等。

use serde::{Deserialize, Serialize};

use crate::{CheckReport, EvidenceRails, ObligationTaskLink, Phase, Plan, Snapshot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceWritebackDeferReason {
    GeneratedMissing,
    GeneratedNotSettled,
    GeneratedNoEvidence,
    GeneratedEpochMismatch,
    InsufficientEvidence,
    TargetMissing,
    TargetObservationMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredWriteback {
    pub link: ObligationTaskLink,
    pub reason: EvidenceWritebackDeferReason,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceWritebackReport {
    pub written: usize,
    pub skipped: usize,
    pub deferred: Vec<DeferredWriteback>,
}

pub fn writeback_obligation_evidence(
    plan: &Plan,
    snapshot: &mut Snapshot,
    links: &[ObligationTaskLink],
) -> EvidenceWritebackReport {
    let mut result = EvidenceWritebackReport::default();
    for link in links {
        let Some(target_spec) = plan.task(&link.target_task) else {
            result.deferred.push(DeferredWriteback {
                link: link.clone(),
                reason: EvidenceWritebackDeferReason::TargetMissing,
            });
            continue;
        };
        let Some(generated_spec) = plan.task(&link.generated_task) else {
            result.deferred.push(DeferredWriteback {
                link: link.clone(),
                reason: EvidenceWritebackDeferReason::GeneratedMissing,
            });
            continue;
        };
        let Some(generated_observation) = snapshot.observations.get(&link.generated_task).cloned()
        else {
            result.deferred.push(DeferredWriteback {
                link: link.clone(),
                reason: EvidenceWritebackDeferReason::GeneratedNotSettled,
            });
            continue;
        };
        if generated_observation.phase != Phase::Settled {
            result.deferred.push(DeferredWriteback {
                link: link.clone(),
                reason: EvidenceWritebackDeferReason::GeneratedNotSettled,
            });
            continue;
        }

        let target_epoch = target_spec.epoch;
        let generated_epoch = generated_spec.epoch;
        let min_assertions = generated_spec.produces.min_assertions;
        let mut items: Vec<(String, CheckReport)> = Vec::new();
        if !generated_observation.evidence_rails.is_empty() {
            let mut seen_stale = false;
            let mut seen_insufficient = false;
            for (rail, entries) in [
                ("support", &generated_observation.evidence_rails.support),
                ("counter", &generated_observation.evidence_rails.counter),
                (
                    "inconclusive",
                    &generated_observation.evidence_rails.inconclusive,
                ),
            ] {
                for item in entries {
                    if item.report.epoch != generated_epoch {
                        seen_stale = true;
                        continue;
                    }
                    if item.report.checks == 0 {
                        continue;
                    }
                    if item.report.checks < min_assertions {
                        seen_insufficient = true;
                        continue;
                    }
                    let mut rebased = item.report.clone();
                    rebased.epoch = target_epoch;
                    items.push((
                        format!("{}:{}:{}", link.generated_task, item.source, rail),
                        rebased,
                    ));
                }
            }
            if items.is_empty() {
                result.deferred.push(DeferredWriteback {
                    link: link.clone(),
                    reason: if seen_stale {
                        EvidenceWritebackDeferReason::GeneratedEpochMismatch
                    } else if seen_insufficient {
                        EvidenceWritebackDeferReason::InsufficientEvidence
                    } else {
                        EvidenceWritebackDeferReason::GeneratedNoEvidence
                    },
                });
                continue;
            }
        } else if let Some(legacy) = &generated_observation.evidence {
            if legacy.epoch != generated_epoch {
                result.deferred.push(DeferredWriteback {
                    link: link.clone(),
                    reason: EvidenceWritebackDeferReason::GeneratedEpochMismatch,
                });
                continue;
            }
            if legacy.checks == 0 {
                result.deferred.push(DeferredWriteback {
                    link: link.clone(),
                    reason: EvidenceWritebackDeferReason::GeneratedNoEvidence,
                });
                continue;
            }
            if legacy.checks < min_assertions {
                result.deferred.push(DeferredWriteback {
                    link: link.clone(),
                    reason: EvidenceWritebackDeferReason::InsufficientEvidence,
                });
                continue;
            }
            let mut rebased = legacy.clone();
            rebased.epoch = target_epoch;
            items.push((format!("{}:evidence", link.generated_task), rebased));
        } else {
            result.deferred.push(DeferredWriteback {
                link: link.clone(),
                reason: EvidenceWritebackDeferReason::GeneratedNoEvidence,
            });
            continue;
        }

        let Some(target_observation) = snapshot.observations.get_mut(&link.target_task) else {
            result.deferred.push(DeferredWriteback {
                link: link.clone(),
                reason: EvidenceWritebackDeferReason::TargetObservationMissing,
            });
            continue;
        };
        if target_observation.evidence_rails.is_empty()
            && let Some(legacy) = target_observation.evidence.clone()
        {
            target_observation
                .evidence_rails
                .push(format!("{}:legacy", link.target_task), legacy);
        }
        for (source, report) in items {
            if has_source(&target_observation.evidence_rails, &source) {
                result.skipped += 1;
                continue;
            }
            target_observation.evidence_rails.push(source, report);
            result.written += 1;
        }
    }
    result
}

fn has_source(rails: &EvidenceRails, source: &str) -> bool {
    rails
        .support
        .iter()
        .chain(&rails.counter)
        .chain(&rails.inconclusive)
        .any(|item| item.source == source)
}
