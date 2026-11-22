use crate::budget::Budget;
use crate::{
    BlockReason, Decision, Error, Ledger, Limits, Observation, Phase, Selection, Snapshot,
    TaskSpec, Ternary, admit,
};

pub(crate) fn select(
    revision: u64,
    tasks: &[TaskSpec],
    predecessors: &[Vec<usize>],
    order: &[usize],
    snapshot: &Snapshot,
    limits: &Limits,
    ledger: &Ledger,
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
            obligations: Vec::new(),
        })
        .collect();
    for &position in order {
        let decision = &mut decisions[position];
        let spec = &tasks[position];
        let observation = observations[position];
        match observation.phase {
            Phase::InFlight => decision.reasons.push(BlockReason::InFlight),
            Phase::Settled if observation.epoch != spec.epoch => {
                // 定义已换 epoch：旧结果作废，按可重新派发处理。
                decision.reasons.push(BlockReason::Superseded {
                    run_epoch: observation.epoch,
                    task_epoch: spec.epoch,
                });
                admit_pending(
                    spec,
                    position,
                    tasks,
                    predecessors,
                    &observations,
                    observation,
                    &mut budget,
                    limits,
                    ledger,
                    decision,
                    &mut selected,
                )?;
            }
            Phase::Settled => {
                // 已完成且结果未被取代：不可派发，但不是「拒绝」。
                // 输入过期时由宿主刷新 freshness 或 bump epoch，内核不擅自重跑。
                decision.reasons.push(BlockReason::Settled);
            }
            Phase::Pending => admit_pending(
                spec,
                position,
                tasks,
                predecessors,
                &observations,
                observation,
                &mut budget,
                limits,
                ledger,
                decision,
                &mut selected,
            )?,
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

#[allow(clippy::too_many_arguments)]
fn admit_pending(
    spec: &TaskSpec,
    position: usize,
    tasks: &[TaskSpec],
    predecessors: &[Vec<usize>],
    observations: &[&Observation],
    observation: &Observation,
    budget: &mut Budget,
    limits: &Limits,
    ledger: &Ledger,
    decision: &mut Decision,
    selected: &mut Vec<String>,
) -> Result<(), Error> {
    let dependencies = dependency_admission(
        spec,
        &predecessors[position],
        tasks,
        observations,
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
    // 过期/未知的输入只能让结论未知，不能升格成「明确拒绝」。
    let freshness_gate = if observation.freshness == Ternary::True {
        Ternary::True
    } else {
        Ternary::Unknown
    };
    let remaining = budget.remaining(position);
    let resources = budget.gate(spec, remaining, limits, &mut decision.reasons)?;
    let effects = effect_gate(&spec.task.id, ledger, &mut decision.reasons);
    decision.admission = admit(
        dependencies,
        observation.authorization,
        resources,
        freshness_gate,
    ) & effects;
    if decision.admission == Ternary::Unknown {
        decision.obligations = crate::obligations_for(&spec.task.id, &decision.reasons);
    }
    if decision.admission == Ternary::True {
        budget.reserve(spec, remaining)?;
        selected.push(spec.task.id.clone());
    }
    Ok(())
}

pub(crate) fn validate_snapshot(
    revision: u64,
    tasks: &[TaskSpec],
    snapshot: &Snapshot,
) -> Result<(), Error> {
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

mod gates;
use gates::{dependency_admission, effect_gate};
