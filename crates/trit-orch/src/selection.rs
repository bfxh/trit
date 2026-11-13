use crate::budget::Budget;
use crate::{
    BlockReason, ClaimSet, Decision, Effect, Error, EvidenceVerdict, Ledger, Limits, Observation,
    Phase, Selection, Snapshot, TaskSpec, Ternary, admit, dependency_gate,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn select(
    revision: u64,
    tasks: &[TaskSpec],
    predecessors: &[Vec<usize>],
    order: &[usize],
    snapshot: &Snapshot,
    limits: &Limits,
    ledger: &Ledger,
    claims: Option<&ClaimSet<'_>>,
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
            granted_tokens: 0,
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
                    claims,
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
                claims,
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
    claims: Option<&ClaimSet<'_>>,
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
    let claim = claim_admission(spec, claims, &mut decision.reasons)?;
    decision.admission = admit(
        dependencies,
        observation.authorization,
        resources,
        freshness_gate,
    ) & effects
        & claim;
    if decision.admission == Ternary::Unknown {
        decision.obligations = crate::obligations_for(&spec.task.id, &decision.reasons);
    }
    if decision.admission == Ternary::True {
        budget.reserve(spec, remaining)?;
        decision.granted_tokens = remaining;
        selected.push(spec.task.id.clone());
    }
    Ok(())
}

/// 效果门：在途效果只能对账，不能靠再次派发绕过。
///
/// 操作 ID 约定与 [`crate::dispatch_ready`] 一致：单效果任务用任务 ID。
/// `Confirmed` 是终态，不允许重发；`Prepared`/`Refuted`/无记录仍交给
/// `dispatch_ready` 做最终意图校验。
fn effect_gate(task: &str, ledger: &Ledger, reasons: &mut Vec<BlockReason>) -> Ternary {
    match ledger.state(task) {
        Some(state @ (Effect::Sending | Effect::Sent | Effect::Unknown)) => {
            reasons.push(BlockReason::Effect { state });
            Ternary::Unknown
        }
        Some(Effect::Confirmed) => {
            reasons.push(BlockReason::Effect {
                state: Effect::Confirmed,
            });
            Ternary::False
        }
        _ => Ternary::True,
    }
}

fn claim_admission(
    spec: &TaskSpec,
    claims: Option<&ClaimSet<'_>>,
    reasons: &mut Vec<BlockReason>,
) -> Result<Ternary, Error> {
    let Some(claims) = claims else {
        return Ok(Ternary::True);
    };
    let Some(paths) = claims.paths_for(&spec.task.id) else {
        reasons.push(BlockReason::Claim {
            verdict: Ternary::Unknown,
        });
        return Ok(Ternary::Unknown);
    };
    if paths.is_empty() {
        return Ok(Ternary::True);
    }
    let verdict = claims.store.gate(paths, &claims.owner, claims.now)?;
    if verdict != Ternary::True {
        reasons.push(BlockReason::Claim { verdict });
    }
    Ok(verdict)
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

fn dependency_admission(
    spec: &TaskSpec,
    predecessors: &[usize],
    tasks: &[TaskSpec],
    observations: &[&Observation],
    reasons: &mut Vec<BlockReason>,
) -> Ternary {
    let mut admission = Ternary::True;
    for (dependency, &predecessor) in spec.task.deps.iter().zip(predecessors) {
        let observation = observations[predecessor];
        let predecessor_epoch = tasks[predecessor].epoch;
        // 只有「已完成、epoch 匹配、输入仍新鲜」的结果才可信；
        // 其余（含 freshness=False）一律 Neither——过期证据既不能证明成功，也不能证明失败。
        let judgment = if observation.phase == Phase::Settled
            && observation.epoch == predecessor_epoch
            && observation.freshness == Ternary::True
        {
            observation
                .evidence
                .as_ref()
                .map(|report| report.judgment(predecessor_epoch))
                .unwrap_or(EvidenceVerdict::Neither)
        } else {
            EvidenceVerdict::Neither
        };
        // 冲突走裁决，不降级成“再测一次”；可选依赖的证据冲突不阻塞下游。
        // 相关性未知时先决定“需不需要”，冲突义务仍保留，不互相遮盖。
        let verdict = judgment.value();
        let relevance_unknown = dependency.required == Ternary::Unknown;
        let conflict = judgment.is_conflicted() && dependency.required != Ternary::False;
        let gate = if relevance_unknown || conflict {
            if relevance_unknown {
                reasons.push(BlockReason::Dependency {
                    task: dependency.task.clone(),
                    required: dependency.required,
                    verdict,
                });
            }
            if conflict {
                reasons.push(BlockReason::Conflict {
                    task: dependency.task.clone(),
                });
            }
            Ternary::Unknown
        } else {
            let gate = dependency_gate(dependency.required, verdict);
            if gate != Ternary::True {
                reasons.push(BlockReason::Dependency {
                    task: dependency.task.clone(),
                    required: dependency.required,
                    verdict,
                });
            }
            gate
        };
        admission = admission & gate;
    }
    admission
}
