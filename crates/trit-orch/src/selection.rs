use crate::budget::Budget;
use crate::{
    BlockReason, Decision, Error, Limits, Observation, Phase, Selection, Snapshot, TaskSpec,
    Ternary, admit, dependency_gate,
};

pub(crate) fn select(
    revision: u64,
    tasks: &[TaskSpec],
    predecessors: &[Vec<usize>],
    order: &[usize],
    snapshot: &Snapshot,
    limits: &Limits,
) -> Result<Selection, Error> {
    validate_snapshot(revision, tasks, snapshot)?;
    if limits.workers == 0 {
        return Err(Error::NoWorkers);
    }
    let observations: Vec<&Observation> = snapshot.observations.values().collect();
    let mut budget = Budget::from_snapshot(tasks, &observations, limits)?;
    let mut selected = Vec::new();
    let mut decisions: Vec<Decision> = tasks
        .iter()
        .map(|spec| Decision {
            task: spec.task.id.clone(),
            admission: Ternary::Unknown,
            reasons: Vec::new(),
        })
        .collect();
    for &position in order {
        let decision = &mut decisions[position];
        let observation = observations[position];
        match observation.phase {
            Phase::InFlight => decision.reasons.push(BlockReason::InFlight),
            Phase::Settled => {
                decision.admission = Ternary::False;
                decision.reasons.push(BlockReason::Settled);
            }
            Phase::Pending => {
                let spec = &tasks[position];
                let dependencies = dependency_admission(
                    spec,
                    &predecessors[position],
                    &observations,
                    revision,
                    &mut decision.reasons,
                );
                if observation.authorization != Ternary::True {
                    decision.reasons.push(BlockReason::Authorization {
                        verdict: observation.authorization,
                    });
                }
                if observation.freshness != Ternary::True {
                    decision.reasons.push(BlockReason::Freshness {
                        verdict: observation.freshness,
                    });
                }
                let resources = budget.gate(spec, limits, &mut decision.reasons)?;
                decision.admission = admit(
                    dependencies,
                    observation.authorization,
                    resources,
                    observation.freshness,
                );
                if decision.admission == Ternary::True {
                    budget.reserve(spec, spec.max_tokens)?;
                    selected.push(spec.task.id.clone());
                }
            }
        }
    }
    Ok(Selection {
        revision,
        selected,
        decisions,
        observed_tokens: budget.observed,
        reserved_tokens: budget.reserved_tokens,
        reserved_resources: budget.resources,
        occupied_workers: budget.workers,
    })
}

fn validate_snapshot(revision: u64, tasks: &[TaskSpec], snapshot: &Snapshot) -> Result<(), Error> {
    if snapshot.revision != revision {
        return Err(Error::RevisionMismatch {
            expected: revision,
            actual: snapshot.revision,
        });
    }
    if !tasks
        .iter()
        .map(|spec| spec.task.id.as_str())
        .eq(snapshot.observations.keys().map(String::as_str))
    {
        let missing = tasks
            .iter()
            .filter(|spec| !snapshot.observations.contains_key(&spec.task.id))
            .map(|spec| spec.task.id.clone())
            .collect();
        let unexpected = snapshot
            .observations
            .keys()
            .filter(|id| tasks.binary_search_by(|spec| spec.task.id.cmp(id)).is_err())
            .cloned()
            .collect();
        return Err(Error::ObservationMismatch {
            missing,
            unexpected,
        });
    }
    Ok(())
}

fn dependency_admission(
    spec: &TaskSpec,
    predecessors: &[usize],
    observations: &[&Observation],
    revision: u64,
    reasons: &mut Vec<BlockReason>,
) -> Ternary {
    let mut admission = Ternary::True;
    for (dependency, &predecessor) in spec.task.deps.iter().zip(predecessors) {
        let observation = observations[predecessor];
        let verdict = if observation.phase == Phase::Settled {
            observation
                .evidence
                .as_ref()
                .map(|report| report.verdict(revision))
                .unwrap_or(Ternary::Unknown)
                & observation.freshness
        } else {
            Ternary::Unknown
        };
        let gate = dependency_gate(dependency.required, verdict);
        if gate != Ternary::True {
            reasons.push(BlockReason::Dependency {
                task: dependency.task.clone(),
                required: dependency.required,
                verdict,
            });
        }
        admission = admission & gate;
    }
    admission
}
