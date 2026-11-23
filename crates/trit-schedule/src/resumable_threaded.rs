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
    state.running += 1;
    if state.started_tasks.insert(position) {
        state.started += 1;
    }
    state.idle = state.idle.saturating_sub(1);
    state.used = state
        .used
        .checked_add(state.requirements[position])
        .expect("预检保证资源相加不溢出");
    state.peak = state.peak.max(state.used);
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
                        state.running = state.running.saturating_sub(1);
                        state.used = state.used.release(state.requirements[position]);
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
                        state.running = state.running.saturating_sub(1);
                        state.used = state.used.release(state.requirements[position]);
                        state.idle += 1;
                        state.preemptions += 1;
                        wake.notify_all();
                        break;
                    }
                    drop(state);
                }
                Progress::Done => {
                    state.running = state.running.saturating_sub(1);
                    state.used = state.used.release(state.requirements[position]);
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
                    state.running = state.running.saturating_sub(1);
                    state.used = state.used.release(state.requirements[position]);
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
mod tests {
    use super::*;
    use crate::Dependency;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Barrier, Mutex};
    use std::time::Duration;

    struct Counter {
        done: usize,
        target: usize,
        log: Option<Arc<Mutex<Vec<usize>>>>,
        active: Option<Arc<AtomicUsize>>,
        max: Option<Arc<AtomicUsize>>,
        barrier: Option<Arc<Barrier>>,
    }

    impl Counter {
        fn new(target: usize) -> Self {
            Self {
                done: 0,
                target,
                log: None,
                active: None,
                max: None,
                barrier: None,
            }
        }

        fn with_log(mut self, log: Arc<Mutex<Vec<usize>>>) -> Self {
            self.log = Some(log);
            self
        }

        fn with_concurrency(mut self, active: Arc<AtomicUsize>, max: Arc<AtomicUsize>) -> Self {
            self.active = Some(active);
            self.max = Some(max);
            self
        }

        fn with_barrier(mut self, barrier: Arc<Barrier>) -> Self {
            self.barrier = Some(barrier);
            self
        }
    }

    impl ResumableTask for Counter {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            if self.done == 0
                && let Some(barrier) = &self.barrier
            {
                barrier.wait();
                std::thread::sleep(Duration::from_millis(100));
            }
            if let (Some(active), Some(max)) = (&self.active, &self.max) {
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(15));
                active.fetch_sub(1, Ordering::SeqCst);
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
            self.done += 1;
            if let Some(log) = &self.log {
                log.lock().unwrap().push(self.done);
            }
            if self.done >= self.target {
                Progress::Done
            } else {
                Progress::Yield
            }
        }
    }

    struct BarrierOnce(Arc<Barrier>);

    impl ResumableTask for BarrierOnce {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            self.0.wait();
            Progress::Done
        }
    }

    fn dep(task: &str) -> Dependency {
        Dependency {
            task: task.to_string(),
            required: Ternary::True,
        }
    }

    #[test]
    fn threaded_preemption_keeps_task_state() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let barrier = Arc::new(Barrier::new(2));
        let low = ResumableSpec::new(
            "low",
            vec![],
            Counter::new(10)
                .with_log(Arc::clone(&log))
                .with_barrier(Arc::clone(&barrier)),
        )
        .with_priority(0)
        .with_requirements(Resources::new(0, 10));
        let seed =
            ResumableSpec::new("seed", vec![], BarrierOnce(Arc::clone(&barrier))).with_priority(10);
        let high = ResumableSpec::new("high", vec![dep("seed")], Counter::new(1))
            .with_priority(20)
            .with_requirements(Resources::new(0, 10));
        let report = run_resumable_threaded(
            vec![low, seed, high],
            RuntimeConfig::new(2).with_capacity(Resources::new(0, 10)),
            CancelToken::new(),
        )
        .unwrap();
        assert_eq!(report.completed, vec!["high", "low", "seed"]);
        assert!(report.preemptions >= 1, "{report:?}");
        assert_eq!(*log.lock().unwrap(), (1..=10).collect::<Vec<_>>());
        assert_eq!(report.metrics.started, 3);
        assert_eq!(report.metrics.steps, report.steps);
    }

    #[test]
    fn two_workers_overlap_steps() {
        let active = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let make = |id: &str| {
            ResumableSpec::new(
                id,
                vec![],
                Counter::new(2).with_concurrency(Arc::clone(&active), Arc::clone(&max)),
            )
        };
        let report = run_resumable_threaded(
            vec![make("a"), make("b")],
            RuntimeConfig::new(2),
            CancelToken::new(),
        )
        .unwrap();
        assert_eq!(report.completed, vec!["a", "b"]);
        assert!(max.load(Ordering::SeqCst) >= 2);
        assert_eq!(report.metrics.started, 2);
    }

    #[test]
    fn cancellation_and_metrics() {
        let token = CancelToken::new();
        token.cancel();
        let report = run_resumable_threaded(
            vec![ResumableSpec::new("a", vec![], Counter::new(3))],
            RuntimeConfig::new(1),
            token,
        )
        .unwrap();
        assert!(report.completed.is_empty());
        assert_eq!(report.cancelled, vec!["a"]);
        assert_eq!(report.metrics.started, 0);
        assert_eq!(report.metrics.cancelled, 1);
    }

    #[test]
    fn task_panic_fails_the_run_instead_of_hanging() {
        // 复审 H06：`step` 的 panic 曾直接穿透 worker——running/资源不归还、
        // 不 notify，其余 worker 与调用方永久等待。现在必须走 Failed 收尾：
        // 归还计数、置 failed、stop、cancel pending、notify_all。
        struct Boom;
        impl ResumableTask for Boom {
            fn step(&mut self, _ctx: &StepContext) -> Progress {
                panic!("boom");
            }
        }

        let report = run_resumable_threaded(
            vec![
                ResumableSpec::new("boom", vec![], Boom),
                ResumableSpec::new("ok", vec![], Counter::new(1)),
            ],
            RuntimeConfig::new(2),
            CancelToken::new(),
        )
        .unwrap();
        let (id, message) = report
            .failed
            .clone()
            .expect("panic 任务必须记为失败而不是挂死");
        assert_eq!(id, "boom");
        assert_eq!(message, "任务 panic");
        assert!(report.completed.contains(&"ok".to_string()), "{report:?}");
    }

    #[test]
    fn failure_after_stop_does_not_promote_dependents() {
        // #634：一个任务 Failed 置 stop 后，threaded 不得再把依赖任务跑完并记
        // completed；依赖只能出现在 cancelled，不能同时落在两个集合里。
        struct Slow;
        impl ResumableTask for Slow {
            fn step(&mut self, _ctx: &StepContext) -> Progress {
                std::thread::sleep(Duration::from_millis(30));
                Progress::Done
            }
        }
        struct FailSet;
        impl ResumableTask for FailSet {
            fn step(&mut self, _ctx: &StepContext) -> Progress {
                std::thread::sleep(Duration::from_millis(20));
                Progress::Failed("boom".to_string())
            }
        }
        let specs = vec![
            ResumableSpec::new("a", vec![], Slow).with_priority(0),
            ResumableSpec::new("b", vec![], FailSet).with_priority(10),
            ResumableSpec::new("c", vec![dep("a")], Counter::new(1)).with_priority(0),
        ];
        let report =
            run_resumable_threaded(specs, RuntimeConfig::new(2), CancelToken::new()).unwrap();
        let (id, message) = report.failed.as_ref().expect("b 必须记为失败");
        assert_eq!(id, "b");
        assert_eq!(message, "boom");
        assert!(!report.completed.contains(&"c".to_string()), "{report:?}");
        assert!(report.cancelled.contains(&"c".to_string()), "{report:?}");
    }
}
