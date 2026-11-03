use serde::{Deserialize, Serialize};

use crate::{BlockReason, Decision, Selection, Ternary};

/// 一批决定按语义分组后的计数（供面板与日志使用）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub selected: usize,
    pub held: usize,
    pub denied: usize,
    pub in_flight: usize,
    pub settled: usize,
}

impl Decision {
    fn is_in_flight(&self) -> bool {
        self.reasons
            .iter()
            .any(|r| matches!(r, BlockReason::InFlight))
    }

    fn is_settled(&self) -> bool {
        self.reasons
            .iter()
            .any(|r| matches!(r, BlockReason::Settled))
    }
}

impl Selection {
    /// 按「在途 / 已结算 / 挂起 / 拒绝 / 已选」分类计数。
    /// 在途与已结算不是「挂起」或「拒绝」，不能混进同一格。
    pub fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for decision in &self.decisions {
            if decision.is_in_flight() {
                counts.in_flight += 1;
            } else if decision.is_settled() {
                counts.settled += 1;
            } else {
                match decision.admission {
                    Ternary::True => counts.selected += 1,
                    Ternary::Unknown => counts.held += 1,
                    Ternary::False => counts.denied += 1,
                }
            }
        }
        counts
    }

    /// 一行状态：修订、批次、挂起/拒绝、在途、worker 与 token 占用。
    /// 这是给面板/日志的短摘要，不是验收结论。
    pub fn brief(&self) -> String {
        let c = self.counts();
        let observed = self
            .observed_tokens
            .fresh
            .saturating_add(self.observed_tokens.cached)
            .saturating_add(self.observed_tokens.output);
        format!(
            "rev={} selected={} held={} denied={} inflight={} settled={} workers={} tokens={}+{}",
            self.revision,
            c.selected,
            c.held,
            c.denied,
            c.in_flight,
            c.settled,
            self.occupied_workers,
            observed,
            self.reserved_tokens,
        )
    }
}
