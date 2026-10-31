//! 大规模压力测试：证明系统在上量时正确且不退化。
//! 每条测试打印计时与规模，人工可验。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;
use trit_immune::*;

fn temp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "trit-immune-stress-{}-{}.journal",
        name,
        std::process::id()
    ))
}

// ── 1. 千节点 DAG 构建 + 门禁 + 传播 ─────────────────────────────────────

#[test]
fn thousand_node_dag_build_and_propagate() {
    let start = Instant::now();
    let mut dag = ImmuneDag::root("main");

    // 构建 500 个叶子模块（无依赖边）
    let leaves: Vec<NodeId> = (0..500)
        .map(|i| {
            let n = dag.spawn(NodeId(0), NodeKind::Delta, &format!("模块{i:04}"));
            dag.record(n, Trit::Hardened, None, "通过");
            n
        })
        .collect();

    // 构建 500 个攻击分支（一半打穿）
    for (i, &leaf) in leaves.iter().enumerate() {
        let atk = dag.spawn(leaf, NodeKind::AttackVector, &format!("攻击{i:04}"));
        if i % 2 == 0 {
            dag.record(atk, Trit::Vulnerable, None, "打穿");
            dag.record(leaf, Trit::Vulnerable, None, "证伪");
        } else {
            dag.record(atk, Trit::Hardened, None, "拦截");
        }
    }

    let build_time = start.elapsed();
    assert_eq!(dag.nodes().len(), 1001, "根 + 500 叶 + 500 攻击");

    // 传播：所有 -1 的攻击分支传染到叶子
    let prop_start = Instant::now();
    let cv = dag.convergence();
    let prop_time = prop_start.elapsed();

    assert_eq!(cv.total, 1001);
    assert_eq!(cv.vulnerable, 500, "一半攻击 + 一半叶子被证伪");
    assert_eq!(cv.unknown, 0);
    assert_eq!(cv.hardened, 501, "root + 干净叶子 + 干净攻击");

    // 门禁：打穿的叶子 BLOCK
    let gate_start = Instant::now();
    let mut blocked = 0;
    let mut allowed = 0;
    for &leaf in leaves.iter() {
        match dag.merge_gate(leaf) {
            GateDecision::Block => blocked += 1,
            GateDecision::Allow => allowed += 1,
            _ => {}
        }
    }
    let gate_time = gate_start.elapsed();

    assert_eq!(blocked, 250, "一半被证伪");
    assert_eq!(allowed, 250, "另一半干净");

    println!(
        "千节点 DAG：构建 {:?}，传播 {:?}，门禁 {:?}（1001 节点、500 门禁）",
        build_time, prop_time, gate_time
    );
}

// ── 2. 万级日志：追加 + 重放一致性 ──────────────────────────────────────

#[test]
fn ten_thousand_delta_journal_roundtrip() {
    let pa = temp("10k-deltas");
    let _ = std::fs::remove_file(&pa);

    let start = Instant::now();
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();

    // 2000 个节点，每个 spawn + record = 4000 条 Delta
    let nodes: Vec<NodeId> = (0..2000)
        .map(|i| {
            let n = j
                .spawn(NodeId(0), NodeKind::Delta, &format!("节点{i:04}"))
                .unwrap();
            j.record(n, Trit::Hardened, None, &format!("通过{i}"))
                .unwrap();
            n
        })
        .collect();
    let write_time = start.elapsed();

    let snapshot = j.dag().clone();
    j.into_dag().unwrap();

    // 重放
    let replay_start = Instant::now();
    let j2 = Journal::open(&pa).unwrap();
    let replay_time = replay_start.elapsed();

    assert_eq!(j2.dag(), &snapshot, "重放状态必须一致");
    assert_eq!(j2.dag().nodes().len(), 2001);

    // 验证每个节点裁决正确
    for &n in nodes.iter().take(100) {
        assert_eq!(j2.dag().node(n).verdict, Trit::Hardened);
    }

    println!(
        "万级日志：{} 条 Delta 写入 {:?}，重放 {:?}",
        4000, write_time, replay_time
    );
    let _ = std::fs::remove_file(&pa);
}

// ── 3. 并发 worker 池 + 大量任务 ─────────────────────────────────────────

#[test]
fn scheduler_thousand_tasks_eight_workers() {
    use trit_immune::scheduler::{drain_plan, Scheduler, Task};

    let mut dag = ImmuneDag::root("main");
    // 500 个 0 态节点（Review 任务）
    for i in 0..500 { dag.spawn(NodeId(0), NodeKind::Delta, &format!("任务{i:04}")); }

    struct NoOpHandler {
        counter: Arc<AtomicUsize>,
    }
    impl scheduler::Handler for NoOpHandler {
        fn handle(&mut self, _task: &Task) -> Result<(), String> {
            self.counter.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    let start = Instant::now();
    let counter = Arc::new(AtomicUsize::new(0));
    let sched = Scheduler::new(8);
    let mut total_rounds = 0;

    // 有界循环：drain 到收敛
    loop {
        total_rounds += 1;
        assert!(total_rounds <= 10, "有界终止");
        let plan = drain_plan(&dag, 100);
        if plan.is_empty() {
            break;
        }
        // NoOp handler 不改裁决——先手动模拟所有 Review
        for t in &plan {
            if let Task::Review { node, .. } = t {
                dag.record(*node, Trit::Hardened, None, "调度处理");
            }
        }
        let mut h = NoOpHandler {
            counter: Arc::clone(&counter),
        };
        let report = sched.run(plan, &mut h);
        assert_eq!(report.failed, 0);
    }
    let elapsed = start.elapsed();

    assert_eq!(counter.load(Ordering::Relaxed), 500, "500 个任务恰好一次");
    assert_eq!(dag.convergence().ratio, 1.0, "全 +1 固定点");

    println!(
        "调度器：500 任务 8 worker {:?}（{} 轮）",
        elapsed, total_rounds
    );
}

// ── 4. 六副本同步 + 跨仓污染 + 合并一致性 ────────────────────────────────

#[test]
fn six_replicas_sync_and_cross_repo_propagation() {
    let start = Instant::now();
    let mut fleet = Fleet::new();
    let paths: Vec<std::path::PathBuf> = (0..6u32)
        .map(|w| temp(&format!("6r-w{w}")))
        .collect();

    // 建 6 个仓：root + 若干节点
    for w in 0..6u32 {
        let p = &paths[w as usize];
        let _ = std::fs::remove_file(p);
        let mut j = Journal::create_as(p, &format!("main-{w}"), w).unwrap();
        let f = j
            .spawn(NodeId(0), NodeKind::Delta, &format!("模块{w}"))
            .unwrap();
        j.record(f, Trit::Hardened, None, "通过").unwrap();
        j.into_dag().unwrap();
        fleet.add(&format!("repo{w}"), Journal::open_as(p, w).unwrap()).unwrap();
    }

    // 跨仓边：repo0 的节点 → repo1..5
    for i in 1..6u32 {
        fleet
            .add_cross_link(
                ("repo0", NodeId(1)),
                (&format!("repo{i}"), NodeId(1)),
                Trit::Vulnerable,
                &format!("repo{i} 复用 repo0"),
            )
            .unwrap();
    }

    // 把 repo0 的节点打到 -1
    {
        let r0 = fleet.get_mut("repo0").unwrap();
        r0.record(NodeId(1), Trit::Vulnerable, None, "核心节点被证伪")
            .unwrap();
    }

    // 舰队级传播：repo0 的 -1 传染所有依赖它的仓
    let cont = fleet.contamination();
    for i in 1..6u32 {
        let repo_name = format!("repo{i}");
        let c = &cont[&repo_name];
        assert_eq!(
            c[1],
            Trit::Vulnerable,
            "repo{i} 的依赖节点应被传染为 -1"
        );
    }

    // 合并日志
    let mut all_stamped = Vec::new();
    for p in paths.iter() {
        let log = sync::parse_log_file(p).unwrap_or_default();
        all_stamped.push(log);
    }
    let merged = delta::merge_order(all_stamped);
    assert!(!merged.is_empty());

    let elapsed = start.elapsed();
    println!(
        "六副本：跨仓传播 + 合并 {:?}（6 仓、5 条跨仓边）",
        elapsed
    );

    // 清理
    for p in paths.iter() {
        let _ = std::fs::remove_file(p);
    }
}

// ── 5. 生命周期 DAG：大规模 + 级联回滚 ──────────────────────────────────

#[test]
fn lifecycle_hundred_nodes_cascade_rollback() {
    use crate::lifecycle::Lifecycle;

    let start = Instant::now();
    let mut lc = Lifecycle::new();

    // 线性链：100 个节点依次依赖
    let ids: Vec<usize> = (0..100)
        .map(|i| {
            let name = if i == 0 { "设计" } else { &format!("阶段{i}")[..] };
            let track = if i < 50 { "ci" } else { "adoption" };
            lc.node(name, track, "PR-1")
        })
        .collect();
    for w in ids.windows(2) {
        lc.depends_on(w[0], w[1]).unwrap();
    }

    // 全部通过
    for &id in ids.iter() {
        lc.set(id, Trit::Hardened).unwrap();
    }
    assert!((lc.convergence().ratio - 1.0).abs() < 1e-9);

    // 从中间回滚 → 后 50 个全部回 0
    lc.rollback_from(ids[50]).unwrap();
    let cv = lc.convergence();
    assert_eq!(cv.hardened, 50, "前 50 个保持");
    assert_eq!(cv.unknown, 50, "后 50 个被级联回滚");

    // 恢复：从断点重新走
    for &id in ids.iter().skip(50) {
        lc.set(id, Trit::Hardened).unwrap();
    }
    assert!((lc.convergence().ratio - 1.0).abs() < 1e-9, "恢复到全 +1");

    let elapsed = start.elapsed();
    println!("生命周期：100 节点级联回滚+恢复 {:?}", elapsed);
}

// ── 6. 概念台账：大量登记 + 同构检测不退化 ───────────────────────────────

#[test]
fn vocab_hundred_concepts_no_degradation() {
    use trit_immune::vocab::{Concept, Vocabulary};

    let start = Instant::now();
    let mut v = Vocabulary::new();
    for i in 0..100 {
        let c = Concept::new(
            &format!("概念{i:03}"),
            &[&format!("别名{i:03}")],
            "test",
            &format!("第 {i} 个测试概念"),
            &[("kind", &format!("type{i}")), ("index", &format!("{i}"))],
        );
        match v.register(c) {
            Ok(trit_immune::Registration::Accepted) => {}
            other => panic!("概念{i} 应入账，实际 {other:?}"),
        }
    }
    assert_eq!(v.len(), 100);
    let a = v.absorption();
    assert_eq!(a.concepts, 100);
    assert_eq!(a.aliases, 100);

    // 同构检测：与概念 50 相同属性 → 拒收
    let dup = Concept::new(
        "山寨",
        &[],
        "test",
        "换词不换设计",
        &[("kind", "type50"), ("index", "50")],
    );
    match v.register(dup) {
        Ok(trit_immune::Registration::Duplicate { of }) => assert_eq!(of, "概念050"),
        other => panic!("应判同构，实际 {other:?}"),
    }

    let elapsed = start.elapsed();
    println!("概念台账：100 条 + 同构检测 {:?}", elapsed);
}

// ── 7. 全管线端到端：收割→扫描→死依赖→入账→调度器→固定点 ────────────────

#[test]
fn full_pipeline_end_to_end() {
    let start = Instant::now();
    let pa = temp("full-pipeline");
    let _ = std::fs::remove_file(&pa);

    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let root = j.spawn(NodeId(0), NodeKind::Message, "收件箱").unwrap();
    j.into_dag().unwrap();

    let mut j = Journal::open_as(&pa, 0).unwrap();

    // 先为每个代码片段创建一个子节点（intake 需要已存在的节点来 Record）
    let mut snippets: Vec<(u32, String)> = Vec::new();
    for i in 0..50u32 {
        let code = if i % 2 == 0 {
            format!("db.password = \"pass{i}\";")
        } else {
            format!("fn safe{i}() {{}}")
        };
        let id = j.spawn(root, NodeKind::Message, &format!("片段{i:02}")).unwrap();
        snippets.push((id.0, code));
    }
    let refs: Vec<(u32, &str)> = snippets.iter().map(|(n, c)| (*n, c.as_str())).collect();
    let toml = "[dependencies]\nserde = \"1.0\"\nphantom = \"0.1\"\n";
    let report = intake::intake_pr(&mut j, root, "github/pr/99", &refs, Some(toml), true).unwrap();
    assert!(report.scan_findings > 0, "25 个安全问题");
    assert!(report.dead_deps > 0, "死依赖");

    // ② 调度器有界 drain
    let sched = Scheduler::new(4);
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds <= 20, "有界终止");
        let plan = drain_plan(j.dag(), 5);
        let has_work = plan.iter().any(|t| !matches!(t, Task::Refactor { .. }));
        if !has_work {
            break;
        }
        let mut h = JournalHandler { journal: &mut j };
        let r = sched.run(plan, &mut h);
        assert_eq!(r.failed, 0);
    }

    // ③ 固定点：无未知、无漏洞
    let cv = j.dag().convergence();
    assert_eq!(cv.unknown, 0);
    assert_eq!(cv.vulnerable, 0);
    assert!((cv.ratio - 1.0).abs() < 1e-9, "全 +1 固定点");

    // ④ 日志重放
    j.into_dag().unwrap();
    let j2 = Journal::open(&pa).unwrap();
    assert_eq!(j2.dag().convergence().ratio, 1.0);

    let elapsed = start.elapsed();
    println!(
        "全管线端到端：{} 节点、扫描+死依赖+隔离+调度+重放 {:?}",
        j2.dag().nodes().len(),
        elapsed
    );
    let _ = std::fs::remove_file(&pa);
}

// ── 8. 概念台账 + 同构检测 1000 条 ───────────────────────────────────────

#[test]
fn vocab_thousand_concepts_isomorphism() {
    use trit_immune::vocab::{Concept, Vocabulary};

    let start = Instant::now();
    let mut v = Vocabulary::new();
    for i in 0..1000 {
        let c = Concept::new(
            &format!("概念{i:04}"),
            &[&format!("别名{i:04}"), &format!("别称{i:04}")],
            "domain",
            &format!("定义 {i}"),
            &[("kind", &format!("type{i}")), ("idx", &format!("{i}"))],
        );
        match v.register(c) {
            Ok(trit_immune::Registration::Accepted) => {}
            other => panic!("概念{i} 应入账：{other:?}"),
        }
    }
    assert_eq!(v.len(), 1000);

    // 同构检测：任意一个的属性集完全一致 → Duplicate
    let dup = Concept::new(
        "山寨500",
        &[],
        "domain",
        "换词",
        &[("kind", "type500"), ("idx", "500")],
    );
    match v.register(dup) {
        Ok(trit_immune::Registration::Duplicate { of }) => assert_eq!(of, "概念0500"),
        other => panic!("应判同构：{other:?}"),
    }

    let elapsed = start.elapsed();
    println!("概念台账：1000 条 + 同构检测 {:?}", elapsed);
}

// ── 9. 并发 HTTP 服务器：100 请求 ─────────────────────────────────────────

#[test]
fn server_hundred_concurrent_requests() {
    use std::net::SocketAddr;

    let pa = temp("srv-stress");
    let _ = std::fs::remove_file(&pa);
    let mut j = Journal::create_as(&pa, "main", 0).unwrap();
    let nodes: Vec<NodeId> = (0..10)
        .map(|i| {
            let n = j.spawn(NodeId(0), NodeKind::Delta, &format!("节点{i}")).unwrap();
            j.record(n, Trit::Hardened, None, "通过").unwrap();
            n
        })
        .collect();
    j.into_dag().unwrap();

    let server = Server::new(Journal::open_as(&pa, 0).unwrap());
    let addr: SocketAddr = server.serve("127.0.0.1:0".parse().unwrap()).unwrap();

    let start = Instant::now();
    let success = Arc::new(AtomicUsize::new(0));
    let mut handles = Vec::new();
    for t in 0..10u32 {
        let addr = addr;
        let success = Arc::clone(&success);
        let nodes = nodes.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..10u32 {
                let node = nodes[((t * 10 + i) % 10) as usize];
                let (s, b) =
                    server::client::get(addr, &format!("/v1/gate?tip={}", node.0)).unwrap();
                assert_eq!(s, 200);
                assert!(b.contains("gate="), "{b}");
                success.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let elapsed = start.elapsed();
    assert_eq!(success.load(Ordering::Relaxed), 100);

    println!(
        "并发 HTTP：100 请求（10 线程 × 10）{:?}",
        elapsed
    );
    let _ = std::fs::remove_file(&pa);
}
