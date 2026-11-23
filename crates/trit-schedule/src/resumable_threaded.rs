//! 可恢复任务的多线程 runtime。
//!
//! 与单线程参考执行器共用 [`ResumableTask`] 协议；worker 在 `Yield` 步骤边界检查是否
//! 应让位给更高优先级任务。被抢占的任务对象放回 ready 队列，状态不会重置。
//! 运行中的 `step` 不会被强杀。

use std::collections::{BTreeMap, BTreeSet, BinaryHeap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};

use trit_core::value::Ternary;

use crate::{
    CancelToken, DeferredTask, MAX_TASKS, Progress, Resources, ResumableError, ResumableReport,
    ResumableSpec, ResumableTask, RuntimeConfig, RuntimeMetrics, ScheduleError, StepContext,
    propagate_unknown_deferrals,
};

struct Shared {
    tasks: Vec<Option<Box<dyn ResumableTask>>>,
    ready: BinaryHeap<(i32, String, usize)>,
    dependents: Vec<Vec<usize>>,
    indegree: Vec<usize>,
    priorities: Vec<i32>,
    requirements: Vec<Resources>,
    ids: Vec<String>,
    workers: usize,
    running: usize,
    idle: usize,
    started: usize,
    started_tasks: BTreeSet<usize>,
    used: Resources,
    capacity: Option<Resources>,
    peak: Resources,
    stop: bool,
    completed: BTreeSet<String>,
    cancelled: BTreeSet<String>,
    failed: Option<(String, String)>,
    deferred: Vec<DeferredTask>,
    preemptions: usize,
    steps: u64,
}

struct Acquired {
    position: usize,
    id: String,
    priority: i32,
    task: Box<dyn ResumableTask>,
}

/// 已认领 worker 槽位与资源的 RAII 守卫。
///
/// 不变量：`held == true` 时 `running` 多占一个槽位、`used` 已计入本任务资源。
/// 无论正常收尾、抢占、取消还是 `step` 之外的 panic unwind，drop 都会归还
/// `running`/资源并 `notify_all`，避免槽位泄漏把其余 worker 永久留在 Condvar。
struct SlotLease<'a> {
    lock: &'a Mutex<Shared>,
    wake: &'a Condvar,
    requirements: Resources,
    held: bool,
}

impl<'a> SlotLease<'a> {
    fn new(lock: &'a Mutex<Shared>, wake: &'a Condvar, requirements: Resources) -> Self {
        Self {
            lock,
            wake,
            requirements,
            held: true,
        }
    }

    /// 在已持有锁的收尾分支中归还槽位与资源并唤醒 peer。
    fn release_locked(&mut self, state: &mut Shared) {
        if !self.held {
            return;
        }
        self.held = false;
        state.running = state.running.saturating_sub(1);
        state.used = state.used.release(self.requirements);
        self.wake.notify_all();
    }
}

impl Drop for SlotLease<'_> {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        self.held = false;
        // unwind 兜底：重新上锁归还（毒化锁按 into_inner 恢复，与 worker 其余路径一致）。
        let mut state = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.running = state.running.saturating_sub(1);
        state.used = state.used.release(self.requirements);
        self.wake.notify_all();
    }
}

/// 多线程执行可恢复任务图。
pub fn run_resumable_threaded(
    specs: Vec<ResumableSpec>,
    config: RuntimeConfig,
    cancel: CancelToken,
) -> Result<ResumableReport, ResumableError> {
    let workers = config.workers;
    let capacity = config.capacity;
    if workers == 0 {
        return Err(ScheduleError::NoWorkers.into());
    }
    if specs.len() > MAX_TASKS {
        return Err(ScheduleError::TooManyTasks(specs.len()).into());
    }
    let n = specs.len();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for (position, spec) in specs.iter().enumerate() {
        if index.insert(spec.id.clone(), position).is_some() {
            return Err(ScheduleError::DuplicateTask(spec.id.clone()).into());
        }
    }
    let mut true_deps: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut direct_unknown: Vec<Option<String>> = vec![None; n];
    for (position, spec) in specs.iter().enumerate() {
        for dependency in &spec.deps {
            let Some(&predecessor) = index.get(&dependency.task) else {
                return Err(ScheduleError::UnknownDependency {
                    task: spec.id.clone(),
                    dependency: dependency.task.clone(),
                }
                .into());
            };
            match dependency.required {
                Ternary::True => true_deps[position].push(predecessor),
                Ternary::Unknown => {
                    if direct_unknown[position].is_none() {
                        direct_unknown[position] = Some(dependency.task.clone());
                    }
                }
                Ternary::False => {}
            }
        }
    }
    let deferred = propagate_unknown_deferrals(&true_deps, direct_unknown, |position| {
        specs[position].id.clone()
    });
    let active_count = deferred.iter().filter(|reason| reason.is_none()).count();
    if let Some(capacity) = capacity {
        for position in 0..n {
            if deferred[position].is_some() {
                continue;
            }
            if !specs[position]
                .requirements
                .fits_in(capacity, Resources::default())
            {
                return Err(ResumableError::Unschedulable {
                    task: specs[position].id.clone(),
                    requirements: specs[position].requirements,
                    capacity,
                });
            }
        }
    }
    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for position in 0..n {
        if deferred[position].is_some() {
            continue;
        }
        for &predecessor in &true_deps[position] {
            if deferred[predecessor].is_none() {
                indegree[position] += 1;
                dependents[predecessor].push(position);
            }
        }
    }
    let mut queue: VecDeque<usize> = (0..n)
        .filter(|&position| deferred[position].is_none() && indegree[position] == 0)
        .collect();
    let mut topological = Vec::with_capacity(active_count);
    while let Some(position) = queue.pop_front() {
        topological.push(position);
        for &next in &dependents[position] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push_back(next);
            }
        }
    }
    if topological.len() != active_count {
        let mut remaining: Vec<&str> = (0..n)
            .filter(|&position| deferred[position].is_none() && !topological.contains(&position))
            .map(|position| specs[position].id.as_str())
            .collect();
        remaining.sort_unstable();
        return Err(ScheduleError::Cycle(remaining.join(", ")).into());
    }
    let mut run_indegree = vec![0usize; n];
    for position in 0..n {
        if deferred[position].is_some() {
            continue;
        }
        for &predecessor in &true_deps[position] {
            if deferred[predecessor].is_none() {
                run_indegree[position] += 1;
            }
        }
    }
    let ids: Vec<String> = specs.iter().map(|spec| spec.id.clone()).collect();
    let priorities: Vec<i32> = specs.iter().map(|spec| spec.priority).collect();
    let requirements: Vec<Resources> = specs.iter().map(|spec| spec.requirements).collect();
    let tasks: Vec<Option<Box<dyn ResumableTask>>> =
        specs.into_iter().map(|spec| Some(spec.task)).collect();
    let deferred_out: Vec<DeferredTask> = deferred
        .iter()
        .enumerate()
        .filter_map(|(position, reason)| {
            reason.clone().map(|reason| DeferredTask {
                id: ids[position].clone(),
                reason,
            })
        })
        .collect();
    let ready: BinaryHeap<(i32, String, usize)> = (0..n)
        .filter(|&position| deferred[position].is_none() && run_indegree[position] == 0)
        .map(|position| (priorities[position], ids[position].clone(), position))
        .collect();
    if active_count == 0 {
        return Ok(ResumableReport {
            workers,
            completed: Vec::new(),
            failed: None,
            cancelled: Vec::new(),
            deferred: deferred_out,
            preemptions: 0,
            steps: 0,
            peak_usage: Resources::default(),
            metrics: RuntimeMetrics {
                started: 0,
                completed: 0,
                failed: 0,
                cancelled: 0,
                deferred: 0,
                preemptions: 0,
                steps: 0,
                peak_usage: Resources::default(),
            },
        });
    }
    let shared = Arc::new((
        Mutex::new(Shared {
            tasks,
            ready,
            dependents,
            indegree: run_indegree,
            priorities,
            requirements,
            ids,
            workers,
            running: 0,
            idle: 0,
            started: 0,
            started_tasks: BTreeSet::new(),
            used: Resources::default(),
            capacity,
            peak: Resources::default(),
            stop: false,
            completed: BTreeSet::new(),
            cancelled: BTreeSet::new(),
            failed: None,
            deferred: deferred_out,
            preemptions: 0,
            steps: 0,
        }),
        Condvar::new(),
    ));
    let effective_workers = workers.min(active_count);
    std::thread::scope(|scope| {
        for _ in 0..effective_workers {
            let shared = Arc::clone(&shared);
            let cancel = cancel.clone();
            scope.spawn(move || worker(shared, cancel));
        }
    });
    let mut state = shared
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let leftover: Vec<String> = state
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(position, task)| task.as_ref().map(|_| state.ids[position].clone()))
        .collect();
    for id in leftover {
        state.cancelled.insert(id);
    }
    let completed: Vec<String> = state.completed.iter().cloned().collect();
    let cancelled: Vec<String> = state.cancelled.iter().cloned().collect();
    let metrics = RuntimeMetrics {
        started: state.started,
        completed: completed.len(),
        failed: usize::from(state.failed.is_some()),
        cancelled: cancelled.len(),
        deferred: state.deferred.len(),
        preemptions: state.preemptions,
        steps: state.steps,
        peak_usage: state.peak,
    };
    Ok(ResumableReport {
        workers,
        completed,
        failed: state.failed.clone(),
        cancelled,
        deferred: state.deferred.clone(),
        preemptions: state.preemptions,
        steps: state.steps,
        peak_usage: state.peak,
        metrics,
    })
}

fn acquire(state: &mut Shared) -> Option<Acquired> {
    // 已置停（失败/取消/溢出）后不再认领新工作：就绪任务直接归 cancelled。
    // 对齐 `runtime/execute.rs` 的 stop 守卫；失败后新 promote 进 ready 的
    // 依赖也在此被排空，避免被空闲 worker 取出执行。
    if state.stop {
        while let Some((_, id, _)) = state.ready.pop() {
            state.cancelled.insert(id);
        }
        return None;
    }
    let &(priority, _, position) = state.ready.peek()?;
    if state.running >= state.workers {
        return None;
    }
    if state
        .capacity
        .is_some_and(|capacity| !state.requirements[position].fits_in(capacity, state.used))
    {
        return None;
    }
    let (_, id, position) = state.ready.pop()?;
    let task = state.tasks[position].take()?;
    Some(Acquired {
        position,
        id,
        priority,
        task,
    })
}

fn might_fit(state: &Shared, position: usize) -> bool {
    state
        .capacity
        .is_none_or(|capacity| state.requirements[position].fits_in(capacity, state.used))
}

fn cancel_pending(state: &mut Shared) {
    while let Some((_, id, _)) = state.ready.pop() {
        state.cancelled.insert(id);
    }
    for (position, task) in state.tasks.iter().enumerate() {
        if task.is_some() && !state.completed.contains(&state.ids[position]) {
            state.cancelled.insert(state.ids[position].clone());
        }
    }
}

fn worker(shared: Arc<(Mutex<Shared>, Condvar)>, cancel: CancelToken) {
    let (lock, wake) = &*shared;
    let mut state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.idle += 1;
    loop {
        if cancel.is_cancelled() && !state.stop {
            state.stop = true;
            cancel_pending(&mut state);
        }
        let acquired = loop {
            if let Some(acquired) = acquire(&mut state) {
                break Some(acquired);
            }
            if state.stop || (state.running == 0 && state.ready.is_empty()) {
                state.idle = state.idle.saturating_sub(1);
                return;
            }
            state = wake
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        };
        let Some(Acquired {
            position,
            id,
            priority,
            mut task,
        }) = acquired
        else {
            continue;
        };
        let requirements = state.requirements[position];
        // 资源预检在 running 计数自增之前：capacity=None 时准入不做累计加法预检，
        // 两个 u32::MAX 需求并发相加仍会溢出。溢出按结构化失败处理（对齐
        // execute.rs:310-319）：置 failed/stop、排空 ready、notify_all，绝不
        // expect panic；此路径不自增 running、不计入 used。
        let Some(new_used) = state.used.checked_add(requirements) else {
            state.failed = Some((id.clone(), "资源累计溢出".to_string()));
            state.stop = true;
            cancel_pending(&mut state);
            wake.notify_all();
            continue;
        };
        state.running += 1;
        if state.started_tasks.insert(position) {
            state.started += 1;
        }
        state.idle = state.idle.saturating_sub(1);
        state.used = new_used;
        state.peak = state.peak.max(state.used);

        // RAII 槽位守卫：任一退出路径（Done/Failed/抢占/取消/panic unwind）都
        // 归还 running 与资源并 notify_all，不靠各分支手动减计数。
        let mut lease = SlotLease::new(lock, wake, requirements);
        drop(state);
        let ctx = StepContext {
            cancel: cancel.clone(),
        };
        loop {
            // 复审 H06：`task.step` 的 panic 曾直接穿透 worker——running/资源
            // 不归还、不 notify，其余 worker 与调用方永久等待。包住并走与
            // `Progress::Failed` 相同的收尾路径（归还、置 failed、stop、notify）。
            let progress = catch_unwind(AssertUnwindSafe(|| task.step(&ctx)))
                .unwrap_or_else(|_| Progress::Failed("任务 panic".to_string()));
            state = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            state.steps += 1;
            match progress {
                Progress::Yield => {
                    if cancel.is_cancelled() || state.stop {
                        lease.release_locked(&mut state);
                        state.cancelled.insert(id);
                        state.idle += 1;
                        wake.notify_all();
                        return;
                    }
                    let should_preempt = state.ready.peek().is_some_and(|(p, _, high)| {
                        *p > priority && (state.idle == 0 || !might_fit(&state, *high))
                    });
                    if should_preempt {
                        state.tasks[position] = Some(task);
                        state.ready.push((priority, id, position));
                        lease.release_locked(&mut state);
                        state.idle += 1;
                        state.preemptions += 1;
                        wake.notify_all();
                        break;
                    }
                    drop(state);
                }
                Progress::Done => {
                    lease.release_locked(&mut state);
                    state.completed.insert(id);
                    // 已置停后不再 promote 后继：依赖任务留在 cancelled，避免同一任务
                    // 同时进入 completed 与 cancelled（对齐 execute.rs:335 的守卫）。
                    if !state.stop {
                        let nexts = state.dependents[position].clone();
                        for next in nexts {
                            state.indegree[next] -= 1;
                            if state.indegree[next] == 0 {
                                let next_priority = state.priorities[next];
                                let next_id = state.ids[next].clone();
                                state.ready.push((next_priority, next_id, next));
                            }
                        }
                    }
                    state.idle += 1;
                    wake.notify_all();
                    break;
                }
                Progress::Failed(message) => {
                    lease.release_locked(&mut state);
                    state.failed = Some((id, message));
                    state.stop = true;
                    cancel_pending(&mut state);
                    state.idle += 1;
                    wake.notify_all();
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
