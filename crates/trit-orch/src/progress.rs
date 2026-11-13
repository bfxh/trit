use serde::{Deserialize, Serialize};

use crate::{Error, EvidenceVerdict, Observation, Phase, Selection, Snapshot, TaskSpec, Ternary};

/// 一轮调度的结构化记录：这一轮选中/挂起/拒绝/在途/已结算各多少。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoundReport {
    pub revision: u64,
    pub selected: usize,
    pub held: usize,
    pub denied: usize,
    pub in_flight: usize,
    pub settled: usize,
    pub observed_tokens: u64,
    pub reserved_tokens: u64,
}

/// 一份计划跑到现在的真实进度。只记已经发生的轮次，不做预测。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    rounds: Vec<RoundReport>,
}

/// 只有全部任务都有「当前 epoch、输入新鲜、判定确定」的结果时才允许给出的完成摘要。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub confirmed: usize,
    pub refuted: usize,
    pub rounds: usize,
}

impl Progress {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从已有的轮次记录恢复（例如宿主重放日志）。
    pub fn from_rounds(rounds: Vec<RoundReport>) -> Self {
        Self { rounds }
    }

    /// 记录一轮选择结果。
    pub fn record(&mut self, selection: &Selection) -> RoundReport {
        let counts = selection.counts();
        let observed = selection
            .observed_tokens
            .fresh
            .saturating_add(selection.observed_tokens.cached)
            .saturating_add(selection.observed_tokens.output);
        let report = RoundReport {
            revision: selection.revision,
            selected: selection.selected.len(),
            held: counts.held,
            denied: counts.denied,
            in_flight: counts.in_flight,
            settled: counts.settled,
            observed_tokens: observed,
            reserved_tokens: selection.reserved_tokens,
        };
        self.rounds.push(report.clone());
        report
    }

    pub fn rounds(&self) -> &[RoundReport] {
        &self.rounds
    }

    /// 累计派发过多少次（所有轮次 `selected` 之和）。
    pub fn dispatched(&self) -> usize {
        self.rounds.iter().map(|round| round.selected).sum()
    }

    /// 一行进度：轮次、累计派发、最后一轮明细。
    pub fn brief(&self) -> String {
        match self.rounds.last() {
            None => "rounds=0 dispatched=0".to_string(),
            Some(last) => format!(
                "rounds={} dispatched={} last=rev{} selected={} held={} denied={} inflight={} settled={} tokens={}+{}",
                self.rounds.len(),
                self.dispatched(),
                last.revision,
                last.selected,
                last.held,
                last.denied,
                last.in_flight,
                last.settled,
                last.observed_tokens,
                last.reserved_tokens,
            ),
        }
    }

    /// 完成闸门：任何未定/未验的任务都让「做完了」判红。
    ///
    /// `confirmed`/`refuted` 只统计「已结算 + 当前 epoch + 输入新鲜 + 判定确定」的结果；
    /// 其余一律计入 `inconclusive`，不带病报完成。
    pub fn finish(&self, tasks: &[TaskSpec], snapshot: &Snapshot) -> Result<Summary, Error> {
        let mut confirmed = 0usize;
        let mut refuted = 0usize;
        let mut pending = 0usize;
        let mut in_flight = 0usize;
        let mut inconclusive = 0usize;
        let mut conflicts = 0usize;
        for spec in tasks {
            let Some(observation) = snapshot.observations.get(&spec.task.id) else {
                pending += 1;
                continue;
            };
            match observation.phase {
                Phase::Pending => pending += 1,
                Phase::InFlight => in_flight += 1,
                Phase::Settled => match decisive(observation, spec) {
                    Some(EvidenceVerdict::Proved) => confirmed += 1,
                    Some(EvidenceVerdict::Refuted) => refuted += 1,
                    Some(EvidenceVerdict::Conflicted) => conflicts += 1,
                    _ => inconclusive += 1,
                },
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
        Ok(Summary {
            confirmed,
            refuted,
            rounds: self.rounds.len(),
        })
    }
}

fn decisive(observation: &Observation, spec: &TaskSpec) -> Option<EvidenceVerdict> {
    if observation.epoch != spec.epoch || observation.freshness != Ternary::True {
        return None;
    }
    observation
        .evidence
        .as_ref()
        .map(|report| report.judgment(spec.epoch))
        .filter(|verdict| *verdict != EvidenceVerdict::Neither)
}
