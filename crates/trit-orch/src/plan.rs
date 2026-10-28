use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{
    Error, Limits, MAX_TASKS, Observation, Phase, PlanPatch, Selection, Snapshot, TaskSpec,
    selection,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    revision: u64,
    tasks: Vec<TaskSpec>,
    predecessors: Vec<Vec<usize>>,
    order: Vec<usize>,
}

impl Plan {
    pub fn new(revision: u64, mut tasks: Vec<TaskSpec>) -> Result<Self, Error> {
        if tasks.len() > MAX_TASKS {
            return Err(Error::TooManyTasks {
                count: tasks.len(),
                max: MAX_TASKS,
            });
        }
        tasks.sort_unstable_by(|left, right| left.task.id.cmp(&right.task.id));
        for spec in &mut tasks {
            spec.task
                .deps
                .sort_unstable_by(|left, right| left.task.cmp(&right.task));
        }
        let predecessors = validate_graph(&tasks)?;
        let mut order: Vec<usize> = (0..tasks.len()).collect();
        order.sort_unstable_by(|&left, &right| {
            let left = &tasks[left].task;
            let right = &tasks[right].task;
            (right.priority, right.duration, &right.id).cmp(&(
                left.priority,
                left.duration,
                &left.id,
            ))
        });
        Ok(Self {
            revision,
            tasks,
            predecessors,
            order,
        })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// 按 ID 取任务定义。
    pub fn task(&self, id: &str) -> Option<&TaskSpec> {
        self.tasks
            .binary_search_by(|spec| spec.task.id.as_str().cmp(id))
            .ok()
            .map(|position| &self.tasks[position])
    }

    /// 以 `patch.base_revision` 为基线打补丁，生成 revision+1 的新计划。
    ///
    /// 保护规则：在途任务不能删除，也不能改定义（只允许原样保留）；
    /// 被替换的任务自动 `epoch+1`，旧证据因此失效、可重新派发。
    pub fn patch(&self, patch: &PlanPatch, snapshot: &Snapshot) -> Result<Self, Error> {
        if patch.base_revision != self.revision {
            return Err(Error::StalePatch {
                base: patch.base_revision,
                current: self.revision,
            });
        }
        selection::validate_snapshot(self.revision, &self.tasks, snapshot)?;

        let mut touched: BTreeSet<&str> = BTreeSet::new();
        for id in patch
            .remove
            .iter()
            .map(String::as_str)
            .chain(patch.add.iter().map(|spec| spec.task.id.as_str()))
            .chain(patch.replace.iter().map(|spec| spec.task.id.as_str()))
        {
            if !touched.insert(id) {
                return Err(Error::PatchDuplicate { id: id.to_owned() });
            }
        }
        for id in &patch.remove {
            self.lookup(id)?;
            if is_in_flight(snapshot, id) {
                return Err(Error::ProtectedInFlight { id: id.clone() });
            }
        }
        for spec in &patch.add {
            if self.task(&spec.task.id).is_some() {
                return Err(Error::PatchAddExists {
                    id: spec.task.id.clone(),
                });
            }
        }
        let mut replacements: BTreeMap<&str, u64> = BTreeMap::new();
        for spec in &patch.replace {
            let current = self.lookup(&spec.task.id)?;
            if is_in_flight(snapshot, &spec.task.id)
                && (!same_definition(&current.task, &spec.task)
                    || current.requirements != spec.requirements
                    || current.max_tokens != spec.max_tokens)
            {
                return Err(Error::ProtectedInFlight {
                    id: spec.task.id.clone(),
                });
            }
            let next = current
                .epoch
                .checked_add(1)
                .ok_or_else(|| Error::EpochOverflow {
                    id: spec.task.id.clone(),
                })?;
            replacements.insert(current.task.id.as_str(), next);
        }

        let removed: BTreeSet<&str> = patch.remove.iter().map(String::as_str).collect();
        let mut tasks: Vec<TaskSpec> = Vec::with_capacity(self.tasks.len() + patch.add.len());
        for spec in &self.tasks {
            if removed.contains(spec.task.id.as_str()) {
                continue;
            }
            match patch
                .replace
                .iter()
                .find(|candidate| candidate.task.id == spec.task.id)
            {
                Some(replacement) => {
                    let mut next = replacement.clone();
                    next.epoch = replacements[spec.task.id.as_str()];
                    tasks.push(next);
                }
                None => tasks.push(spec.clone()),
            }
        }
        tasks.extend(patch.add.iter().cloned());
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(Error::RevisionOverflow)?;
        Self::new(revision, tasks)
    }

    fn lookup(&self, id: &str) -> Result<&TaskSpec, Error> {
        self.task(id)
            .ok_or_else(|| Error::PatchTargetMissing { id: id.to_owned() })
    }

    pub fn select(&self, snapshot: &Snapshot, limits: &Limits) -> Result<Selection, Error> {
        selection::select(
            self.revision,
            &self.tasks,
            &self.predecessors,
            &self.order,
            snapshot,
            limits,
        )
    }
}

fn is_in_flight(snapshot: &Snapshot, id: &str) -> bool {
    matches!(
        snapshot.observations.get(id),
        Some(Observation {
            phase: Phase::InFlight,
            ..
        })
    )
}

fn same_definition(current: &crate::Task, proposed: &crate::Task) -> bool {
    current.id == proposed.id
        && current.duration == proposed.duration
        && current.priority == proposed.priority
        && current.deps == proposed.deps
}

fn task_index(tasks: &[TaskSpec]) -> Result<BTreeMap<&str, usize>, Error> {
    let mut index = BTreeMap::new();
    for (position, spec) in tasks.iter().enumerate() {
        let id = &spec.task.id;
        if id.trim().is_empty() {
            return Err(Error::BlankTaskId { id: id.clone() });
        }
        if index.insert(id.as_str(), position).is_some() {
            return Err(Error::DuplicateTask { id: id.clone() });
        }
    }
    Ok(index)
}

fn validate_graph(tasks: &[TaskSpec]) -> Result<Vec<Vec<usize>>, Error> {
    let index = task_index(tasks)?;
    let mut predecessors = Vec::with_capacity(tasks.len());
    let mut dependents = vec![Vec::new(); tasks.len()];
    for (position, spec) in tasks.iter().enumerate() {
        let mut previous = None;
        let mut incoming = Vec::with_capacity(spec.task.deps.len());
        for dependency in &spec.task.deps {
            if previous == Some(&dependency.task) {
                return Err(Error::DuplicateDependency {
                    task: spec.task.id.clone(),
                    dependency: dependency.task.clone(),
                });
            }
            previous = Some(&dependency.task);
            let &predecessor =
                index
                    .get(dependency.task.as_str())
                    .ok_or_else(|| Error::UnknownDependency {
                        task: spec.task.id.clone(),
                        dependency: dependency.task.clone(),
                    })?;
            incoming.push(predecessor);
            dependents[predecessor].push(position);
        }
        predecessors.push(incoming);
    }
    validate_acyclic(tasks, &predecessors, &dependents)?;
    Ok(predecessors)
}

fn validate_acyclic(
    tasks: &[TaskSpec],
    predecessors: &[Vec<usize>],
    dependents: &[Vec<usize>],
) -> Result<(), Error> {
    let mut remaining: Vec<usize> = predecessors.iter().map(Vec::len).collect();
    let mut ready: VecDeque<usize> = remaining
        .iter()
        .enumerate()
        .filter_map(|(position, &count)| (count == 0).then_some(position))
        .collect();
    while let Some(position) = ready.pop_front() {
        for &dependent in &dependents[position] {
            remaining[dependent] = remaining[dependent]
                .checked_sub(1)
                .expect("each validated edge is removed once");
            if remaining[dependent] == 0 {
                ready.push_back(dependent);
            }
        }
    }
    let blocked: Vec<String> = tasks
        .iter()
        .zip(remaining)
        .filter(|(_, count)| *count != 0)
        .map(|(spec, _)| spec.task.id.clone())
        .collect();
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(Error::Cycle { blocked })
    }
}
