use std::collections::{BTreeMap, VecDeque};

use crate::{Error, Limits, MAX_TASKS, Selection, Snapshot, TaskSpec, selection};

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
