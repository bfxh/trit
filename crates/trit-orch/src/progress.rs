use serde::{Deserialize, Serialize};

use crate::Selection;

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
}

mod decisive;
mod finish;
pub(crate) use decisive::decisive;
