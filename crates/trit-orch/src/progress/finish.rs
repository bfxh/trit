use std::collections::BTreeMap;

use crate::{
    Error, EvidenceSpec, EvidenceVerdict, Observation, Phase, Receipt, Snapshot, TaskSpec, Ternary,
};

use super::{Progress, Summary, decisive};

mod legacy;

impl Progress {
    /// 带可信回执的完成闸门：`confirmed`/`refuted` 只统计「已结算 + 当前 epoch +
    /// 输入新鲜 + 判定确定」的结果，并且每条 evidence 必须与同 task 的 executor
    /// `Receipt` 重算出的 `CheckReport` 完全一致。
    ///
    /// 这是对编排幻觉的硬门：模型可以在 snapshot 里写一个 `Passed` 的
    /// `CheckReport`，但没有受信 executor 回执时不能报完成。
    pub fn finish_with_receipts(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        receipts: &BTreeMap<String, Receipt>,
        spec: EvidenceSpec,
    ) -> Result<Summary, Error> {
        self.finish_inner(tasks, snapshot, Some((receipts, spec)))
    }

    pub(super) fn finish_inner(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        attest: Option<(&BTreeMap<String, Receipt>, EvidenceSpec)>,
    ) -> Result<Summary, Error> {
        let mut confirmed = 0usize;
        let mut refuted = 0usize;
        let mut pending = 0usize;
        let mut in_flight = 0usize;
        let mut inconclusive = 0usize;
        let mut conflicts = 0usize;
        let mut decisive_observations: Vec<(&str, &Observation)> = Vec::new();
        for spec in tasks {
            let Some(observation) = snapshot.observations.get(&spec.task.id) else {
                pending += 1;
                continue;
            };
            match observation.phase {
                Phase::Pending => pending += 1,
                Phase::InFlight => in_flight += 1,
                Phase::Settled if observation.authorization == Ternary::True => {
                    match decisive(observation, spec) {
                        Some(EvidenceVerdict::Proved) => {
                            confirmed += 1;
                            decisive_observations.push((spec.task.id.as_str(), observation));
                        }
                        Some(EvidenceVerdict::Refuted) => {
                            refuted += 1;
                            decisive_observations.push((spec.task.id.as_str(), observation));
                        }
                        Some(EvidenceVerdict::Conflicted) => conflicts += 1,
                        _ => inconclusive += 1,
                    }
                }
                Phase::Settled => inconclusive += 1,
            }
        }
        if pending + in_flight + inconclusive + conflicts > 0 {
            return Err(Error::Incomplete {
                pending,
                in_flight,
                inconclusive,
                conflicts,
            });
        }

        if let Some((receipts, evidence_spec)) = attest {
            let mut unattested = Vec::new();
            for (task, observation) in decisive_observations {
                let Some(receipt) = receipts.get(task) else {
                    unattested.push(task.to_owned());
                    continue;
                };
                let Some(evidence) = observation.evidence.as_ref() else {
                    return Err(Error::ReceiptMismatch {
                        task: task.to_owned(),
                    });
                };
                let expected = receipt.report(&evidence_spec);
                if expected.epoch != evidence.epoch
                    || expected.checks != evidence.checks
                    || expected.result != evidence.result
                {
                    return Err(Error::ReceiptMismatch {
                        task: task.to_owned(),
                    });
                }
            }
            if !unattested.is_empty() {
                return Err(Error::UnattestedEvidence {
                    count: unattested.len(),
                    tasks: unattested,
                });
            }
        }

        Ok(Summary {
            confirmed,
            refuted,
            rounds: self.rounds.len(),
        })
    }
}
