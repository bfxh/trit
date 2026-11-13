//! 多智能体总线仲裁：HighZ 弃权、同值一致、相反值冲突并列出全部驱动方。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use trit_core::tristate::TriState;

use crate::EvidenceVerdict;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDriver {
    pub owner: String,
    pub value: TriState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDispute {
    pub lane: String,
    /// 全部非 HighZ 的驱动方，按 owner 排序。
    pub drivers: Vec<LaneDriver>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaneState {
    Agreed(TriState),
    Conflicted(LaneDispute),
}

impl LaneState {
    pub fn evidence_verdict(&self) -> EvidenceVerdict {
        match self {
            Self::Agreed(TriState::High) => EvidenceVerdict::Proved,
            Self::Agreed(TriState::Low) => EvidenceVerdict::Refuted,
            Self::Agreed(TriState::HighZ) => EvidenceVerdict::Neither,
            Self::Conflicted(_) => EvidenceVerdict::Conflicted,
        }
    }

    pub fn is_conflicted(&self) -> bool {
        matches!(self, Self::Conflicted(_))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneReport {
    pub lanes: BTreeMap<String, LaneState>,
    pub conflicts: Vec<LaneDispute>,
}

/// 解析一组事实/权属/决策车道。
///
/// 对每条车道：没有 active 驱动 → HighZ；全部同值 → 该值；出现相反值 → 冲突，
/// 并把所有 active 驱动方列出来。车道按名字排序，结果确定。
pub fn resolve_lanes(lanes: impl IntoIterator<Item = (String, Vec<LaneDriver>)>) -> LaneReport {
    let mut lanes: Vec<(String, Vec<LaneDriver>)> = lanes.into_iter().collect();
    lanes.sort_by(|left, right| left.0.cmp(&right.0));

    let mut report = LaneReport::default();
    for (lane, drivers) in lanes {
        let mut active: Vec<LaneDriver> = drivers
            .into_iter()
            .filter(|driver| driver.value != TriState::HighZ)
            .collect();
        active.sort_by(|left, right| left.owner.cmp(&right.owner));

        let state = if active.is_empty() {
            LaneState::Agreed(TriState::HighZ)
        } else if active.iter().all(|driver| driver.value == active[0].value) {
            LaneState::Agreed(active[0].value)
        } else {
            let dispute = LaneDispute {
                lane: lane.clone(),
                drivers: active,
            };
            report.conflicts.push(dispute.clone());
            LaneState::Conflicted(dispute)
        };
        report.lanes.insert(lane, state);
    }
    report
}
