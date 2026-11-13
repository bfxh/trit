//! 多源 Belnap 双轨证据：支持轨、反对轨、未定轨。
//!
//! `Conflicted` 单条报告会同时进入两轨；派生时只看当前 epoch 且断言数 > 0 的项。

use serde::{Deserialize, Serialize};

use crate::{CheckReport, CheckResult, EvidenceVerdict};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceItem {
    pub source: String,
    pub report: CheckReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRails {
    pub support: Vec<EvidenceItem>,
    pub counter: Vec<EvidenceItem>,
    pub inconclusive: Vec<EvidenceItem>,
}

impl EvidenceRails {
    pub fn push(&mut self, source: impl Into<String>, report: CheckReport) {
        let item = EvidenceItem {
            source: source.into(),
            report,
        };
        match item.report.result {
            CheckResult::Passed => self.support.push(item),
            CheckResult::Refuted => self.counter.push(item),
            CheckResult::Inconclusive => self.inconclusive.push(item),
            CheckResult::Conflicted => {
                self.support.push(item.clone());
                self.counter.push(item);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.support.is_empty() && self.counter.is_empty() && self.inconclusive.is_empty()
    }

    pub fn len(&self) -> usize {
        self.support.len() + self.counter.len() + self.inconclusive.len()
    }

    pub fn judgment(&self, expected_epoch: u64) -> EvidenceVerdict {
        let support = self
            .support
            .iter()
            .any(|item| current(&item.report, expected_epoch));
        let counter = self
            .counter
            .iter()
            .any(|item| current(&item.report, expected_epoch));
        match (support, counter) {
            (true, true) => EvidenceVerdict::Conflicted,
            (true, false) => EvidenceVerdict::Proved,
            (false, true) => EvidenceVerdict::Refuted,
            (false, false) => EvidenceVerdict::Neither,
        }
    }
}

fn current(report: &CheckReport, expected_epoch: u64) -> bool {
    report.epoch == expected_epoch && report.checks > 0
}
