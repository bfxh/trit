use std::collections::BTreeMap;

use crate::{
    Error, EvidenceSpec, EvidenceVerdict, Observation, Phase, Receipt, ReceiptProvenance, Snapshot,
    TaskSpec, Ternary,
};

use crate::progress::decisive;
use crate::progress::{Progress, Summary};

#[path = "check.rs"]
mod check;

/// 完成闸对回执来源的信任档位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReceiptTrust {
    /// 只接受宿主亲自测量的回执；执行器自报一律拒绝。
    HostMeasuredOnly,
    /// 显式允许自报回执（外部已有独立信任锚时使用）。
    AllowSelfReported,
}

/// 一次完成核验需要的回执上下文。
pub(super) struct Attestation<'a> {
    pub(super) receipts: &'a BTreeMap<String, Receipt>,
    pub(super) spec: EvidenceSpec,
    pub(super) trust: ReceiptTrust,
    pub(super) nonce: Option<&'a str>,
}

impl Progress {
    /// 带可信回执的完成闸门：`confirmed`/`refuted` 只统计「已结算 + 当前 epoch +
    /// 输入新鲜 + 判定确定」的结果，并且每条 evidence 必须与同 task 的**宿主测量**
    /// `Receipt` 重算出的 `CheckReport` 完全一致。
    ///
    /// 这是对编排幻觉的硬门：模型可以在 snapshot 里写一个 `Passed` 的
    /// `CheckReport`，但既没有宿主测量的回执、又拿不出与回执一致的断言计数时，
    /// 不能报完成。
    ///
    /// 与旧版的差别：自报回执（`ReceiptProvenance::SelfReported`）在这里一律
    /// 拒绝。执行器/模型能写进 JSON 的数字不再具有证据效力——可信根只能是宿主
    /// 自己 spawn 执行器后读到的事实。
    pub fn finish_with_receipts(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        receipts: &BTreeMap<String, Receipt>,
        spec: EvidenceSpec,
    ) -> Result<Summary, Error> {
        self.finish_inner(
            tasks,
            snapshot,
            Some(Attestation {
                receipts,
                spec,
                trust: ReceiptTrust::HostMeasuredOnly,
                nonce: None,
            }),
        )
    }

    /// 与 [`Self::finish_with_receipts`] 相同，但要求每条宿主测量回执都携带
    /// 本次 run 的挑战（`nonce`），防止跨轮重放上一轮的真回执。
    pub fn finish_with_receipts_for_nonce(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        receipts: &BTreeMap<String, Receipt>,
        spec: EvidenceSpec,
        expected_nonce: &str,
    ) -> Result<Summary, Error> {
        self.finish_inner(
            tasks,
            snapshot,
            Some(Attestation {
                receipts,
                spec,
                trust: ReceiptTrust::HostMeasuredOnly,
                nonce: Some(expected_nonce),
            }),
        )
    }

    /// 显式逃生舱：接受自报回执。
    ///
    /// 只有调用方在编排链之外已经建立了独立信任锚（例如宿主亲自验过外部执行器
    /// 的签名/身份）时才应使用；默认入口不会走到这里。
    pub fn finish_with_self_reported_receipts(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        receipts: &BTreeMap<String, Receipt>,
        spec: EvidenceSpec,
    ) -> Result<Summary, Error> {
        self.finish_inner(
            tasks,
            snapshot,
            Some(Attestation {
                receipts,
                spec,
                trust: ReceiptTrust::AllowSelfReported,
                nonce: None,
            }),
        )
    }

    pub(super) fn finish_inner(
        &self,
        tasks: &[TaskSpec],
        snapshot: &Snapshot,
        attest: Option<Attestation<'_>>,
    ) -> Result<Summary, Error> {
        let mut confirmed = 0usize;
        let mut refuted = 0usize;
        let mut pending = 0usize;
        let mut in_flight = 0usize;
        let mut inconclusive = 0usize;
        let mut conflicts = 0usize;
        let mut decisive_observations: Vec<(&str, &Observation, &TaskSpec)> = Vec::new();
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
                            decisive_observations.push((spec.task.id.as_str(), observation, spec));
                        }
                        Some(EvidenceVerdict::Refuted) => {
                            refuted += 1;
                            decisive_observations.push((spec.task.id.as_str(), observation, spec));
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

        if let Some(attest) = attest {
            let mut unattested = Vec::new();
            for (task, observation, spec) in decisive_observations {
                let Some(receipt) = attest.receipts.get(task) else {
                    unattested.push(task.to_owned());
                    continue;
                };
                if attest.trust == ReceiptTrust::HostMeasuredOnly
                    && receipt.provenance != ReceiptProvenance::HostMeasured
                {
                    return Err(Error::ReceiptSelfReported {
                        task: task.to_owned(),
                    });
                }
                if let Some(expected) = attest.nonce
                    && receipt.nonce != expected
                {
                    return Err(Error::ReceiptNonceMismatch {
                        task: task.to_owned(),
                    });
                }
                // 计划身份也必须由宿主钉住：缺绑定、revision 或任务定义摘要
                // 任一不符，都说明这条回执属于另一份计划。自报逃生舱已由调用方
                // 显式承担信任，不额外要求 plan 绑定。
                if attest.trust == ReceiptTrust::HostMeasuredOnly {
                    check::plan(task, receipt, snapshot, spec, attest.spec.min_assertions)?;
                }
                // authorization / freshness / usage 也必须有宿主判定，且与快照一致：
                // 只钉 evidence 会让模型靠自报授权/新鲜度/低报用量解锁完成。
                check::observation(
                    task,
                    "authorization",
                    receipt.authorization,
                    observation.authorization,
                )?;
                check::observation(task, "freshness", receipt.freshness, observation.freshness)?;
                check::observation(task, "usage", receipt.usage, observation.usage)?;
                let Some(evidence) = observation.evidence.as_ref() else {
                    return Err(Error::ReceiptMismatch {
                        task: task.to_owned(),
                    });
                };
                let expected = receipt.report(&attest.spec);
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
            rounds: self.rounds().len(),
        })
    }
}
