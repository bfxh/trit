//! 义务 → PlanPatch。
//!
//! 把任意 kind 的义务变成任务；目标必须是 Pending，先在 PlanPatch 里给目标补一条
//! 对生成任务的依赖。生成任务原生声明 `resolves`（目标判定）与 `produces`
//! （最少断言数）。InFlight / 非 Pending / 目标缺失 / 生成任务已存在的义务进入
//! deferred，交给宿主决定。Claim 还要求目标已声明认领路径；路径与按 kind 覆盖的
//! 资源需求随 link 下发给宿主。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    AdmissionObligation, ClaimSet, Dependency, EvidenceSpec, ObligationKind, Phase, Plan,
    PlanPatch, Resources, Snapshot, Task, TaskGuidance, TaskSpec, Ternary,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationTaskPolicy {
    pub requirements: Resources,
    pub min_tokens: u64,
    /// 按义务类型覆盖默认资源需求；缺省回退到 `requirements`。
    #[serde(default)]
    pub requirements_by_kind: BTreeMap<ObligationKind, Resources>,
    /// 生成任务要求的证据规格；默认至少 1 条断言。
    #[serde(default = "default_produces")]
    pub produces: EvidenceSpec,
}

fn default_produces() -> EvidenceSpec {
    EvidenceSpec { min_assertions: 1 }
}

impl Default for ObligationTaskPolicy {
    fn default() -> Self {
        Self {
            requirements: Resources::new(1, 1),
            min_tokens: 1,
            requirements_by_kind: BTreeMap::new(),
            produces: default_produces(),
        }
    }
}

impl ObligationTaskPolicy {
    pub fn task_id(obligation: &AdmissionObligation) -> String {
        format!("ob:{}", obligation.id)
    }

    pub fn requirements_for(&self, kind: ObligationKind) -> Resources {
        self.requirements_by_kind
            .get(&kind)
            .copied()
            .unwrap_or(self.requirements)
    }

    pub fn task_spec(&self, obligation: &AdmissionObligation) -> TaskSpec {
        let objective = if obligation.detail.trim().is_empty() {
            format!(
                "为 {} 补齐 {} 证据",
                obligation.task,
                obligation.kind.as_str()
            )
        } else {
            obligation.detail.clone()
        };
        TaskSpec {
            task: Task {
                id: Self::task_id(obligation),
                duration: obligation.cost.wall.min(u64::from(u32::MAX)) as u32,
                priority: obligation.priority.min(i32::MAX as u32) as i32,
                deps: Vec::new(),
            },
            resolves: vec![obligation.task.clone()],
            produces: self.produces,
            effects: Vec::new(),
            guidance: TaskGuidance {
                objective,
                ..TaskGuidance::default()
            },
            requirements: self.requirements_for(obligation.kind),
            max_tokens: obligation.cost.tokens.max(self.min_tokens),
            epoch: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationTaskLink {
    pub generated_task: String,
    pub target_task: String,
    pub obligation: String,
    pub kind: ObligationKind,
    /// 目标任务已声明的认领路径（排序去重），供宿主决定生成任务的范围。
    /// Claim 生成任务不得再用同一组路径做 claim gate，否则会自锁。
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObligationPlanDeferReason {
    TargetMissing,
    TargetInFlight,
    TargetNotPending,
    TaskExists,
    NoClaimPaths,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredObligation {
    pub obligation: AdmissionObligation,
    pub reason: ObligationPlanDeferReason,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObligationPlan {
    pub patch: PlanPatch,
    pub links: Vec<ObligationTaskLink>,
    pub deferred: Vec<DeferredObligation>,
}

impl ObligationPlan {
    pub fn is_empty(&self) -> bool {
        self.patch.add.is_empty() && self.patch.replace.is_empty()
    }
}

pub fn plan_patch_for_obligations(
    plan: &Plan,
    snapshot: &Snapshot,
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    policy: &ObligationTaskPolicy,
) -> ObligationPlan {
    plan_patch_inner(plan, snapshot, None, obligations, policy)
}

/// 带认领上下文的补丁：生成任务与 link 会带上目标任务的已声明路径。
pub fn plan_patch_for_obligations_with_claims(
    plan: &Plan,
    snapshot: &Snapshot,
    claims: &ClaimSet<'_>,
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    policy: &ObligationTaskPolicy,
) -> ObligationPlan {
    plan_patch_inner(plan, snapshot, Some(claims), obligations, policy)
}

fn plan_patch_inner(
    plan: &Plan,
    snapshot: &Snapshot,
    claims: Option<&ClaimSet<'_>>,
    obligations: impl IntoIterator<Item = AdmissionObligation>,
    policy: &ObligationTaskPolicy,
) -> ObligationPlan {
    let mut patch = PlanPatch {
        base_revision: plan.revision(),
        add: Vec::new(),
        remove: Vec::new(),
        replace: Vec::new(),
    };
    let mut replacements: BTreeMap<String, TaskSpec> = BTreeMap::new();
    let mut links = Vec::new();
    let mut deferred = Vec::new();
    let mut seen = BTreeSet::new();

    for obligation in obligations {
        if !seen.insert(obligation.id.clone()) {
            continue;
        }
        let Some(target) = plan.task(&obligation.task) else {
            deferred.push(DeferredObligation {
                obligation,
                reason: ObligationPlanDeferReason::TargetMissing,
            });
            continue;
        };
        match snapshot
            .observations
            .get(&obligation.task)
            .map(|observation| observation.phase)
        {
            Some(Phase::InFlight) => {
                deferred.push(DeferredObligation {
                    obligation,
                    reason: ObligationPlanDeferReason::TargetInFlight,
                });
                continue;
            }
            Some(Phase::Pending) => {}
            _ => {
                deferred.push(DeferredObligation {
                    obligation,
                    reason: ObligationPlanDeferReason::TargetNotPending,
                });
                continue;
            }
        }

        let generated_id = ObligationTaskPolicy::task_id(&obligation);
        if plan.task(&generated_id).is_some() {
            deferred.push(DeferredObligation {
                obligation,
                reason: ObligationPlanDeferReason::TaskExists,
            });
            continue;
        }
        let paths = declared_paths(claims, &obligation.task);
        if obligation.kind == ObligationKind::Claim && paths.is_empty() {
            deferred.push(DeferredObligation {
                obligation,
                reason: ObligationPlanDeferReason::NoClaimPaths,
            });
            continue;
        }
        let replacement = replacements
            .entry(obligation.task.clone())
            .or_insert_with(|| target.clone());
        if replacement
            .task
            .deps
            .iter()
            .any(|dependency| dependency.task == generated_id)
        {
            continue;
        }
        replacement.task.deps.push(Dependency {
            task: generated_id.clone(),
            required: Ternary::True,
        });
        patch.add.push(policy.task_spec(&obligation));
        links.push(ObligationTaskLink {
            generated_task: generated_id,
            target_task: obligation.task.clone(),
            obligation: obligation.id.clone(),
            kind: obligation.kind,
            paths,
        });
    }

    patch
        .add
        .sort_by(|left, right| left.task.id.cmp(&right.task.id));
    patch.replace = replacements.into_values().collect();
    links.sort_by(|left, right| left.generated_task.cmp(&right.generated_task));
    ObligationPlan {
        patch,
        links,
        deferred,
    }
}

fn declared_paths(claims: Option<&ClaimSet<'_>>, task: &str) -> Vec<String> {
    let Some(paths) = claims.and_then(|set| set.paths_for(task)) else {
        return Vec::new();
    };
    let mut paths = paths.to_vec();
    paths.sort_unstable();
    paths.dedup();
    paths
}
