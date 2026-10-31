//! 本地调度器：把"智能体干活"收编成**有界循环**（GOAL.md 药方三的执行器）。
//!
//! - 任务队列从 DAG 状态**派生**（`drain_plan`）：0 态 → Review、-1 → Fix、
//!   隔离 → Review、设计 smell → Refactor——队列有限，永远有底；
//! - worker 池并发消费（scoped threads + 共享队列），每个任务**恰好执行一次**；
//! - 处理器可插拔：内置 `JournalHandler` 兜底替身（复核/修复直接落账），
//!   真实部署替换为智能体/人工——接口不变；
//! - 终止性：队列有限 + 动作使状态单调（复核解除隔离/落锤、修复落补丁），
//!   ⇒ 重复 drain 到计划为空即到达固定点（测试钉死）。

use crate::dag::{ImmuneDag, NodeId, NodeKind};
use crate::journal::Journal;
use crate::trit::Trit;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// 一单位智能体劳动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Task {
    /// 0 态/被隔离节点：复核（human_review 是唯一出口）。
    Review { node: NodeId, title: String },
    /// 确证漏洞：出补丁 + 复验。
    Fix { node: NodeId, evidence: Option<String> },
    /// 设计 smell：重构建议（不改裁决，只留痕）。
    Refactor { node: NodeId, fan_in: usize },
}

/// 从 DAG 状态派生任务队列（有限——队列长度由当前状态决定，不由工作量决定）。
/// `max_fan_in`：设计层的耦合阈值（超过即 Refactor 候选）。
pub fn drain_plan(dag: &ImmuneDag, max_fan_in: usize) -> Vec<Task> {
    let mut tasks = Vec::new();
    for n in dag.nodes() {
        if n.quarantined {
            tasks.push(Task::Review {
                node: n.id,
                title: n.title.clone(),
            });
            continue;
        }
        match n.verdict {
            Trit::Vulnerable => tasks.push(Task::Fix {
                node: n.id,
                evidence: n
                    .history
                    .iter()
                    .rev()
                    .find_map(|e| e.evidence.as_ref())
                    .map(|e| format!("{}", e.vector)),
                }),
            Trit::Unknown => tasks.push(Task::Review {
                node: n.id,
                title: n.title.clone(),
            }),
            Trit::Hardened => {}
        }
    }
    for id in dag.design_smells(max_fan_in) {
        let fan_in = dag.coupling(id).fan_in;
        tasks.push(Task::Refactor { node: id, fan_in });
    }
    tasks
}

/// 任务处理器（可插拔：替身 / 智能体 / 人工代理）。
pub trait Handler: Send {
    fn handle(&mut self, task: &Task) -> Result<(), String>;
}

/// 执行报告。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WorkReport {
    pub total: usize,
    pub executed: usize,
    pub failed: usize,
}

/// 本地调度器：worker 池并发消费任务队列，每任务恰好执行一次。
#[derive(Debug, Clone, Copy)]
pub struct Scheduler {
    pub workers: usize,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self { workers: 4 }
    }
}

impl Scheduler {
    pub fn new(workers: usize) -> Self {
        Self { workers: workers.max(1) }
    }

    /// 并发执行一轮任务（scoped threads：处理器可借用本地状态）。
    pub fn run(&self, tasks: Vec<Task>, handler: &mut dyn Handler) -> WorkReport {
        let total = tasks.len();
        let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
        let executed = Arc::new(AtomicUsize::new(0));
        let failed = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            let handler = Arc::new(Mutex::new(handler));
            for _ in 0..self.workers.max(1) {
                let queue = Arc::clone(&queue);
                let executed = Arc::clone(&executed);
                let failed = Arc::clone(&failed);
                let handler = Arc::clone(&handler);
                scope.spawn(move || loop {
                    let task = { queue.lock().expect("任务队列锁中毒").pop_front() };
                    let Some(task) = task else { break };
                    let ok = handler
                        .lock()
                        .expect("处理器锁中毒")
                        .handle(&task)
                        .is_ok();
                    if ok {
                        executed.fetch_add(1, Ordering::Relaxed);
                    } else {
                        failed.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
        });
        WorkReport {
            total,
            executed: executed.load(Ordering::Relaxed),
            failed: failed.load(Ordering::Relaxed),
        }
    }
}

/// 兜底替身处理器：把任务结果写回免疫网络（复核 = human_review；
/// 修复 = 补丁子节点 + 复验；真实部署替换为智能体/人工，接口不变）。
pub struct JournalHandler<'a> {
    pub journal: &'a mut Journal,
}

impl Handler for JournalHandler<'_> {
    fn handle(&mut self, task: &Task) -> Result<(), String> {
        match task {
            Task::Review { node, .. } => {
                // 复核 = human_review：解除隔离 + 落锤（0 态唯一出口）
                self.journal
                    .human_review(*node, Trit::Hardened, "调度复核通过（替身）")
                    .map_err(|e| e.to_string())
            }
            Task::Fix { node, .. } => {
                let patch = self
                    .journal
                    .spawn(*node, NodeKind::Patch, "调度补丁（替身）")
                    .map_err(|e| e.to_string())?;
                self.journal
                    .record(patch, Trit::Hardened, None, "调度补丁通过（替身）")
                    .map_err(|e| e.to_string())?;
                self.journal
                    .record(*node, Trit::Hardened, None, "调度修复复验（替身）")
                    .map_err(|e| e.to_string())
            }
            Task::Refactor { node, fan_in } => {
                // 重构建议不改裁决，只留痕
                let v = self.journal.dag().node(*node).verdict;
                self.journal
                    .record(
                        *node,
                        v,
                        None,
                        &format!("调度：重构建议（扇入 {fan_in}）已记录"),
                    )
                    .map_err(|e| e.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 恰好一次执行的计数处理器。
    struct CountingHandler {
        seen: Mutex<Vec<usize>>,
        fail_on: Option<usize>,
    }

    impl Handler for CountingHandler {
        fn handle(&mut self, task: &Task) -> Result<(), String> {
            let Task::Refactor { fan_in, .. } = task else {
                return Err("计数处理器只收 Refactor".into());
            };
            self.seen.lock().unwrap().push(*fan_in);
            if self.fail_on == Some(*fan_in) {
                Err("故意失败".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn tasks_execute_exactly_once_across_workers() {
        let tasks: Vec<Task> = (0..20usize)
            .map(|i| Task::Refactor {
                node: NodeId(i as u32),
                fan_in: i,
            })
            .collect();
        let mut handler = CountingHandler {
            seen: Mutex::new(Vec::new()),
            fail_on: None,
        };
        let report = Scheduler::new(4).run(tasks, &mut handler as &mut dyn Handler);
        assert_eq!(
            report,
            WorkReport {
                total: 20,
                executed: 20,
                failed: 0
            }
        );
        let mut seen = handler.seen.lock().unwrap().clone();
        seen.sort_unstable();
        assert_eq!(seen, (0..20).collect::<Vec<_>>(), "每任务恰好一次");
    }

    #[test]
    fn failures_are_counted_not_swallowed() {
        let tasks: Vec<Task> = (0..5usize)
            .map(|i| Task::Refactor {
                node: NodeId(i as u32),
                fan_in: i,
            })
            .collect();
        let mut handler = CountingHandler {
            seen: Mutex::new(Vec::new()),
            fail_on: Some(2),
        };
        let report = Scheduler::new(2).run(tasks, &mut handler as &mut dyn Handler);
        assert_eq!(
            report,
            WorkReport {
                total: 5,
                executed: 4,
                failed: 1
            }
        );
    }
}
