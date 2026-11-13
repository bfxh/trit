//! 未定质量：`Σ weight(Unknown) + 2 × weight(Conflict)`。
//!
//! 当前每个任务权重为 1；判定过期、epoch 不匹配、缺证据都按 Unknown 计。
//! Conflict 仍按 2 计，因为它需要裁决而不是简单补证。

use serde::{Deserialize, Serialize};

use crate::{EvidenceVerdict, Snapshot, TaskSpec, Ternary};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndecidedMass {
    pub unknown: u64,
    pub conflicts: u64,
    pub mass: u64,
}

impl UndecidedMass {
    pub fn is_zero(&self) -> bool {
        self.mass == 0
    }

    /// P4 验收：新一轮的未定质量必须严格下降。
    pub fn decreased_from(&self, previous: &Self) -> bool {
        self.mass < previous.mass
    }
}

pub fn undecided_mass(tasks: &[TaskSpec], snapshot: &Snapshot) -> UndecidedMass {
    let mut mass = UndecidedMass::default();
    for spec in tasks {
        let Some(observation) = snapshot.observations.get(&spec.task.id) else {
            mass.unknown += 1;
            continue;
        };
        if observation.epoch != spec.epoch || observation.freshness != Ternary::True {
            mass.unknown += 1;
            continue;
        }
        let verdict = observation
            .evidence
            .as_ref()
            .map_or(EvidenceVerdict::Neither, |report| {
                report.judgment(spec.epoch)
            });
        match verdict {
            EvidenceVerdict::Proved | EvidenceVerdict::Refuted => {}
            EvidenceVerdict::Neither => mass.unknown += 1,
            EvidenceVerdict::Conflicted => mass.conflicts += 1,
        }
    }
    mass.mass = mass
        .unknown
        .saturating_add(mass.conflicts.saturating_mul(2));
    mass
}
