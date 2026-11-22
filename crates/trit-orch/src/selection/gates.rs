use crate::{
    BlockReason, Effect, EvidenceVerdict, Ledger, Observation, Phase, TaskSpec, Ternary,
    dependency_gate,
};

/// 效果门：在途效果只能对账，不能靠再次派发绕过。
///
/// 操作 ID 约定与 [`crate::dispatch_ready`] 一致：单效果任务用任务 ID。
/// `Confirmed` 是终态，不允许重发；`Prepared`/`Refuted`/无记录仍交给
/// `dispatch_ready` 做最终意图校验。
pub(super) fn effect_gate(task: &str, ledger: &Ledger, reasons: &mut Vec<BlockReason>) -> Ternary {
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

pub(super) fn dependency_admission(
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
