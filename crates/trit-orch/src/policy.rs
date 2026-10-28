use serde::{Deserialize, Serialize};

use crate::{Ledger, Limits, Selection, TaskSpec};

/// 候选策略只能**提议**，不能改内核决定。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// 建议额外派发的任务（内核没选中）。
    #[serde(default)]
    pub extra: Vec<String>,
    /// 建议不要派发的任务（内核选中了）。
    #[serde(default)]
    pub skip: Vec<String>,
    /// 建议的派发顺序；只影响顺序，不改变集合。
    #[serde(default)]
    pub order: Option<Vec<String>>,
    /// 建议停止。
    #[serde(default)]
    pub stop: bool,
}

/// 影子策略看到的只读上下文。
pub struct ShadowContext<'a> {
    pub selection: &'a Selection,
    pub tasks: &'a [TaskSpec],
    pub ledger: &'a Ledger,
    pub limits: &'a Limits,
}

/// 候选策略接口。被测策略可以是确定性启发式，也可以是以后训练出来的三值模型。
pub trait ShadowPolicy {
    fn name(&self) -> &'static str;
    fn propose(&self, context: &ShadowContext<'_>) -> Proposal;
}

/// 对照组：完全跟随内核，用来证明影子通道本身不产生偏差。
pub struct Baseline;

impl ShadowPolicy for Baseline {
    fn name(&self) -> &'static str {
        "baseline"
    }

    fn propose(&self, _context: &ShadowContext<'_>) -> Proposal {
        Proposal::default()
    }
}

/// 只调顺序：按 duration 降序（同 duration 按 ID 升序），不动集合。
pub struct LongestFirst;

impl ShadowPolicy for LongestFirst {
    fn name(&self) -> &'static str {
        "longest_first"
    }

    fn propose(&self, context: &ShadowContext<'_>) -> Proposal {
        let mut order: Vec<(u32, &str)> = context
            .selection
            .selected
            .iter()
            .map(|id| {
                let duration = context
                    .tasks
                    .iter()
                    .find(|spec| &spec.task.id == id)
                    .map_or(0, |spec| spec.task.duration);
                (duration, id.as_str())
            })
            .collect();
        order.sort_unstable_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
        Proposal {
            order: Some(order.into_iter().map(|(_, id)| id.to_owned()).collect()),
            ..Proposal::default()
        }
    }
}

/// 激进策略：把内核没选中的任务全提上来，用来检验安全记账会不会把危险建议算成“同意”。
pub struct Eager;

impl ShadowPolicy for Eager {
    fn name(&self) -> &'static str {
        "eager"
    }

    fn propose(&self, context: &ShadowContext<'_>) -> Proposal {
        let selected: std::collections::BTreeSet<&str> = context
            .selection
            .selected
            .iter()
            .map(String::as_str)
            .collect();
        Proposal {
            extra: context
                .tasks
                .iter()
                .map(|spec| spec.task.id.as_str())
                .filter(|id| !selected.contains(id))
                .map(str::to_owned)
                .collect(),
            ..Proposal::default()
        }
    }
}

/// 每个任务的 bottom-level：自身 duration + 下游最长链。
///
/// 边规则：`required` 为 True/False 的边都算「后继预计会跑」；`Unknown` 的边不计——
/// 需求未定，不能拿它给任务加权。缺 ID 的边忽略（策略是只读建议，不能因此 panic）。
pub fn bottom_levels(tasks: &[TaskSpec]) -> std::collections::BTreeMap<String, u64> {
    use std::collections::BTreeMap;
    let index: BTreeMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(position, spec)| (spec.task.id.as_str(), position))
        .collect();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); tasks.len()];
    for (position, spec) in tasks.iter().enumerate() {
        for dependency in &spec.task.deps {
            if dependency.required == crate::Ternary::Unknown {
                continue;
            }
            if let Some(&predecessor) = index.get(dependency.task.as_str()) {
                successors[predecessor].push(position);
            }
        }
    }
    let mut bottom: Vec<Option<u64>> = vec![None; tasks.len()];
    for root in 0..tasks.len() {
        // 迭代式后序，避免长链递归爆栈；遇到环（结构上不应出现）按 0 贡献处理。
        let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
        let mut on_stack = vec![false; tasks.len()];
        on_stack[root] = true;
        while let Some(&mut (node, ref mut child)) = stack.last_mut() {
            if *child < successors[node].len() {
                let next = successors[node][*child];
                *child += 1;
                if bottom[next].is_none() && !on_stack[next] {
                    on_stack[next] = true;
                    stack.push((next, 0));
                }
                continue;
            }
            let longest = successors[node]
                .iter()
                .filter_map(|&next| bottom[next])
                .max()
                .unwrap_or(0);
            bottom[node] = Some(u64::from(tasks[node].task.duration).saturating_add(longest));
            on_stack[node] = false;
            stack.pop();
        }
    }
    tasks
        .iter()
        .zip(bottom)
        .map(|(spec, level)| (spec.task.id.clone(), level.unwrap_or(0)))
        .collect()
}

/// 关键路径优先：下游链越长越先派（同长度按 ID 升序）。只调顺序，不改批次。
pub struct CriticalPath;

impl ShadowPolicy for CriticalPath {
    fn name(&self) -> &'static str {
        "critical_path"
    }

    fn propose(&self, context: &ShadowContext<'_>) -> Proposal {
        let levels = bottom_levels(context.tasks);
        let mut order: Vec<(u64, &str)> = context
            .selection
            .selected
            .iter()
            .map(|id| (levels.get(id).copied().unwrap_or(0), id.as_str()))
            .collect();
        order.sort_unstable_by(|left, right| right.0.cmp(&left.0).then(left.1.cmp(right.1)));
        Proposal {
            order: Some(order.into_iter().map(|(_, id)| id.to_owned()).collect()),
            ..Proposal::default()
        }
    }
}

/// 按名字取内置策略。
pub fn builtin(name: &str) -> Option<Box<dyn ShadowPolicy>> {
    match name {
        "baseline" => Some(Box::new(Baseline)),
        "longest_first" => Some(Box::new(LongestFirst)),
        "eager" => Some(Box::new(Eager)),
        "critical_path" => Some(Box::new(CriticalPath)),
        _ => None,
    }
}
