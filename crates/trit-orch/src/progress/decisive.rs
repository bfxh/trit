use crate::{EvidenceVerdict, Observation, TaskSpec, Ternary};

pub(crate) fn decisive(observation: &Observation, spec: &TaskSpec) -> Option<EvidenceVerdict> {
    if observation.epoch != spec.epoch || observation.freshness != Ternary::True {
        return None;
    }
    observation
        .evidence
        .as_ref()
        .map(|report| report.judgment(spec.epoch))
        .filter(|verdict| *verdict != EvidenceVerdict::Neither)
}
