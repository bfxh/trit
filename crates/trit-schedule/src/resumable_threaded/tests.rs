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
    //
    // The old fixture raced on wall-clock sleeps (a=30ms vs b=20ms): a loaded
    // runner could schedule b late, let a finish, and legitimately promote c
    // before the failure landed. Gate a on b's failure flag instead, so the
    // test asserts the invariant, not the runner's scheduling latency.
    let b_failed = Arc::new(AtomicUsize::new(0));
    struct Slow(Arc<AtomicUsize>);
    impl ResumableTask for Slow {
        fn step(&mut self, _ctx: &StepContext) -> Progress {
            while self.0.load(Ordering::SeqCst) == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
            Progress::Done
        }
    }
    struct FailSet(Arc<AtomicUsize>);
    impl ResumableTask for FailSet {
        fn step(&mut self, ctx: &StepContext) -> Progress {
            // Mark cancellation before the failure is committed, so the other
            // worker's Done cannot race the scheduler lock and promote c.
            ctx.cancel.cancel();
            self.0.store(1, Ordering::SeqCst);
            Progress::Failed("boom".to_string())
        }
    }
    let specs = vec![
        ResumableSpec::new("a", vec![], Slow(b_failed.clone())).with_priority(0),
        ResumableSpec::new("b", vec![], FailSet(b_failed)).with_priority(10),
        ResumableSpec::new("c", vec![dep("a")], Counter::new(1)).with_priority(0),
    ];
    let report = run_resumable_threaded(specs, RuntimeConfig::new(2), CancelToken::new()).unwrap();
    let (id, message) = report.failed.as_ref().expect("b 必须记为失败");
    assert_eq!(id, "b");
    assert_eq!(message, "boom");
    assert!(!report.completed.contains(&"a".to_string()), "{report:?}");
    assert!(report.cancelled.contains(&"a".to_string()), "{report:?}");
    assert!(!report.completed.contains(&"c".to_string()), "{report:?}");
    assert!(report.cancelled.contains(&"c".to_string()), "{report:?}");
}

struct HoldThenDone {
    done: usize,
    hold_ms: u64,
}

impl ResumableTask for HoldThenDone {
    fn step(&mut self, _ctx: &StepContext) -> Progress {
        if self.done == 0 {
            std::thread::sleep(Duration::from_millis(self.hold_ms));
        }
        self.done += 1;
        if self.done >= 2 {
            Progress::Done
        } else {
            Progress::Yield
        }
    }
}

#[test]
fn resource_overflow_without_capacity_fails_instead_of_panicking() {
    // #635：capacity=None 下两个 u32::MAX 需求并发相加溢出，不得走 expect
    // panic；必须在有限时间内返回结构化失败，槽位不泄漏、peer 不挂在 Condvar。
    // 堆序保证 b 先被弹出并占满 used，a 被第二个 worker 弹出时必然溢出。
    let maxed = Resources::new(u32::MAX, 0);
    let specs = vec![
        ResumableSpec::new(
            "b",
            vec![],
            HoldThenDone {
                done: 0,
                hold_ms: 100,
            },
        )
        .with_priority(10)
        .with_requirements(maxed),
        ResumableSpec::new("a", vec![], Counter::new(1))
            .with_priority(0)
            .with_requirements(maxed),
    ];
    let report = run_resumable_threaded(specs, RuntimeConfig::new(2), CancelToken::new()).unwrap();
    let (id, message) = report
        .failed
        .as_ref()
        .expect("溢出必须记为结构化失败而不是 panic/挂死");
    assert_eq!(id, "a");
    assert_eq!(message, "资源累计溢出");
    // 溢出的 a 不得进入 completed；在途的 b 在步间边界归 cancelled。
    assert!(!report.completed.contains(&"a".to_string()), "{report:?}");
    assert!(!report.completed.contains(&"b".to_string()), "{report:?}");
    assert!(report.cancelled.contains(&"b".to_string()), "{report:?}");
}

#[test]
fn slots_reusable_after_resource_overflow() {
    // #635：溢出失败后 running 与资源必须归零、worker 槽位未泄漏；同一执行器
    // 再次提交小任务可以正常跑完。
    let maxed = Resources::new(u32::MAX, 0);
    let overflow = vec![
        ResumableSpec::new(
            "b",
            vec![],
            HoldThenDone {
                done: 0,
                hold_ms: 100,
            },
        )
        .with_priority(10)
        .with_requirements(maxed),
        ResumableSpec::new("a", vec![], Counter::new(1))
            .with_priority(0)
            .with_requirements(maxed),
    ];
    let first =
        run_resumable_threaded(overflow, RuntimeConfig::new(2), CancelToken::new()).unwrap();
    assert!(first.failed.is_some(), "{first:?}");
    assert_eq!(first.metrics.started, 1, "{first:?}");

    let second = run_resumable_threaded(
        vec![ResumableSpec::new("ok", vec![], Counter::new(1))],
        RuntimeConfig::new(2),
        CancelToken::new(),
    )
    .unwrap();
    assert_eq!(second.completed, vec!["ok"], "{second:?}");
    assert_eq!(second.metrics.started, 1);
}
