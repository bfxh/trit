//! #634 回归：失败置停前后的 Done 不得 promote 依赖。
//!
//! 放在集成测试里，避免源文件模块预算把新增夹具记到 `resumable_threaded`。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use trit_core::value::Ternary;
use trit_schedule::{
    CancelToken, Dependency, Progress, ResumableSpec, ResumableTask, RuntimeConfig, StepContext,
    run_resumable_threaded,
};

fn dep(task: &str) -> Dependency {
    Dependency {
        task: task.to_string(),
        required: Ternary::True,
    }
}

struct WaitForFailure(Arc<AtomicUsize>);

impl ResumableTask for WaitForFailure {
    fn step(&mut self, _ctx: &StepContext) -> Progress {
        while self.0.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Progress::Done
    }
}

struct FailAndCancel(Arc<AtomicUsize>);

impl ResumableTask for FailAndCancel {
    fn step(&mut self, ctx: &StepContext) -> Progress {
        // 先让取消可见，再返回 Failed；覆盖失败 worker 尚未拿锁置 stop 的窗口。
        ctx.cancel.cancel();
        self.0.store(1, Ordering::SeqCst);
        Progress::Failed("boom".to_string())
    }
}

struct OneStep;

impl ResumableTask for OneStep {
    fn step(&mut self, _ctx: &StepContext) -> Progress {
        Progress::Done
    }
}

#[test]
fn failure_after_stop_does_not_promote_dependents() {
    let b_failed = Arc::new(AtomicUsize::new(0));
    let specs = vec![
        ResumableSpec::new("a", vec![], WaitForFailure(b_failed.clone())).with_priority(0),
        ResumableSpec::new("b", vec![], FailAndCancel(b_failed)).with_priority(10),
        ResumableSpec::new("c", vec![dep("a")], OneStep).with_priority(0),
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
