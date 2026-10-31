//! 本地调度器：并发恰好一次、DAG 收敛到固定点、Journal 端到端 suspects 清零。

use trit_immune::scheduler::{drain_plan, JournalHandler, Scheduler, Task};
use trit_immune::*;

/// DAG 替身处理器（scoped threads 借用本地状态）。
struct DagHandler<'a> {
    dag: &'a mut ImmuneDag,
}

impl scheduler::Handler for DagHandler<'_> {
    fn handle(&mut self, task: &Task) -> Result<(), String> {
        match task {
            Task::Review { node, .. } => {
                // 复核 = human_review（解除隔离 + 落锤，0 态唯一出口）
                self.dag
                    .human_review(*node, Trit::Hardened, "调度复核通过（替身）");
                Ok(())
            }
            Task::Fix { node, .. } => {
                let patch = self.dag.spawn(*node, NodeKind::Patch, "调度补丁（替身）");
                self.dag
                    .record(patch, Trit::Hardened, None, "调度补丁通过（替身）");
                self.dag
                    .record(*node, Trit::Hardened, None, "调度修复复验（替身）");
                Ok(())
            }
            Task::Refactor { node, fan_in } => {
                let v = self.dag.node(*node).verdict;
                self.dag
                    .record(*node, v, None, &format!("调度：重构建议（扇入 {fan_in}）已记录"));
                Ok(())
            }
        }
    }
}

#[test]
fn drain_reaches_fixed_point_on_dag() {
    let mut dag = ImmuneDag::root("main");
    let vuln = dag.spawn(NodeId(0), NodeKind::Delta, "破模块");
    let atk = dag.spawn(vuln, NodeKind::AttackVector, "攻击");
    dag.record(atk, Trit::Vulnerable, None, "打穿");
    dag.record(vuln, Trit::Vulnerable, None, "证伪");
    let _ = dag.spawn(NodeId(0), NodeKind::Delta, "未测一");
    let _ = dag.spawn(NodeId(0), NodeKind::Delta, "未测二");
    let q = dag.spawn(NodeId(0), NodeKind::Delta, "被隔离");
    dag.record(q, Trit::Hardened, None, "原本通过");
    dag.quarantine(q, "测试隔离");
    let hub = dag.spawn(NodeId(0), NodeKind::Delta, "枢纽");
    for i in 0..3 {
        let x = dag.spawn(NodeId(0), NodeKind::Delta, &format!("依赖{i}"));
        dag.cross_link(x, hub, Trit::Hardened, "依赖");
    }

    let sched = Scheduler::new(4);
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds <= 3, "有界：破模块轮 → 攻击轮 → 收尾轮");
        let plan = drain_plan(&dag, 2);
        // 收敛判据只看 Review/Fix——Refactor 是给人看的设计建议（0 态主权），不阻断
        let has_work = plan
            .iter()
            .any(|t| !matches!(t, Task::Refactor { .. }));
        if !has_work {
            assert!(
                plan.iter().any(|t| matches!(t, Task::Refactor { .. })),
                "枢纽的设计建议应保留"
            );
            break;
        }
        let mut h = DagHandler { dag: &mut dag };
        let report = sched.run(plan, &mut h);
        assert_eq!(report.failed, 0, "{report:?}");
    }

    // 固定点：无未知、无漏洞、无隔离，全 +1
    let cv = dag.convergence();
    assert_eq!(cv.unknown, 0);
    assert_eq!(cv.vulnerable, 0);
    assert_eq!(cv.quarantined, 0);
    assert!((cv.ratio - 1.0).abs() < 1e-9, "收敛到全 +1 固定点");
}

#[test]
fn journal_handler_drains_suspects_end_to_end() {
    let pa = std::env::temp_dir().join(format!(
        "trit-immune-sched-{}.journal",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&pa);

    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let _ = j.spawn(NodeId(0), NodeKind::Delta, "未测").unwrap();
    let v = j.spawn(NodeId(0), NodeKind::Delta, "已破").unwrap();
    let atk = j.spawn(v, NodeKind::AttackVector, "攻击").unwrap();
    j.record(atk, Trit::Vulnerable, None, "打穿")
        .expect("record");
    j.record(v, Trit::Vulnerable, None, "证伪")
        .expect("record");
    j.into_dag().unwrap();

    let mut j = Journal::open_as(&pa, 0).unwrap();
    assert_eq!(j.dag().suspects().len(), 1, "未测节点在 0 态队列");

    let sched = Scheduler::new(2);
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds <= 3, "有界：破模块轮 → 攻击轮");
        let plan = drain_plan(j.dag(), 10);
        if plan
            .iter()
            .all(|t| matches!(t, Task::Refactor { .. }))
        {
            break;
        }
        let mut h = JournalHandler { journal: &mut j };
        let report = sched.run(plan, &mut h);
        assert_eq!(report.failed, 0, "{report:?}");
    }

    // 固定点：0 态队列清空、无漏洞
    assert!(j.dag().suspects().is_empty(), "0 态队列清空");
    assert_eq!(j.dag().convergence().vulnerable, 0);
    assert!((j.dag().convergence().ratio - 1.0).abs() < 1e-9);
    j.into_dag().unwrap();

    let _ = std::fs::remove_file(&pa);
}
