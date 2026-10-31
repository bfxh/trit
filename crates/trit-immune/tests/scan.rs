//! 静态扫描路线：校准协议（永不 +1）、折叠最坏者胜、抑制跳过、降级可审计、日志持久化。

use trit_immune::*;

fn temp_journal(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

#[test]
fn dispositive_blocks_heuristic_freezes_clean_touches_nothing() {
    let mut dag = ImmuneDag::root("main");
    let cfg = dag.spawn(NodeId(0), NodeKind::Delta, "配置加载");
    let ui = dag.spawn(NodeId(0), NodeKind::Delta, "渲染");
    let pure = dag.spawn(NodeId(0), NodeKind::Delta, "纯函数库");
    for id in [cfg, ui, pure] {
        dag.record(id, Trit::Hardened, None, "自身回归通过");
    }
    assert_eq!(dag.merge_gate(cfg), GateDecision::Allow);

    let report = scan::run_scan(
        &[
            (cfg, "db.password = \"hunter2\";"),
            (ui, "el.innerHTML = userInput;"),
            (pure, "fn add(a: i64, b: i64) -> i64 { a + b }"),
        ],
        scan::RULES,
        &[],
    );
    assert_eq!(report.findings.len(), 2);
    for d in report.to_deltas("TRI") {
        dag.apply(&d).unwrap();
    }

    // 规则即漏洞本体 → -1 → BLOCK
    assert_eq!(dag.node(cfg).verdict, Trit::Vulnerable);
    assert_eq!(dag.merge_gate(cfg), GateDecision::Block);
    // 启发式 → 0 → FREEZE（进红队队列）
    assert_eq!(dag.node(ui).verdict, Trit::Unknown);
    assert_eq!(dag.merge_gate(ui), GateDecision::Freeze);
    // 清白节点纹丝不动：扫描既没有打穿它，也没有替它担保
    assert_eq!(dag.node(pure).verdict, Trit::Hardened);
    assert_eq!(dag.merge_gate(pure), GateDecision::Allow);
}

#[test]
fn fold_takes_the_worst_across_rules() {
    let report = scan::run_scan(
        &[(NodeId(1), "secret = \"k\"; eval(input);")],
        scan::RULES,
        &[],
    );
    let folds = report.fold_by_node();
    assert_eq!(folds.len(), 1);
    assert_eq!(folds[0].verdict, Trit::Vulnerable, "Dispositive 压过 Heuristic");
    assert_eq!(folds[0].rules.len(), 2);
}

#[test]
fn suppressed_nodes_are_skipped_not_whitewashed() {
    let report = scan::run_scan(
        &[
            (NodeId(1), "api_key = \"sk-123\";"),
            (NodeId(2), "password = \"x\";"),
        ],
        scan::RULES,
        &[NodeId(1)],
    );
    assert_eq!(report.suppressed, 1);
    assert_eq!(report.scanned, 1);
    assert!(report.findings.iter().all(|f| f.node != NodeId(1)));
    assert_eq!(report.findings.len(), 1);
}

#[test]
fn scanner_downgrade_is_auditable() {
    let mut dag = ImmuneDag::root("main");
    let cfg = dag.spawn(NodeId(0), NodeKind::Delta, "配置");
    dag.record(cfg, Trit::Hardened, None, "自身回归通过");
    let report = scan::run_scan(&[(cfg, "password = \"hunter2\";")], scan::RULES, &[]);
    for d in report.to_deltas("TRI") {
        dag.apply(&d).unwrap();
    }
    let h = &dag.node(cfg).history;
    assert_eq!(h.len(), 2);
    assert_eq!(h[0].verdict, Trit::Hardened);
    assert_eq!(h[1].verdict, Trit::Vulnerable, "扫描器把 +1 拉回 -1");
    assert!(h[1].note.contains("TRI001-HARDCODED-SECRET"));
}

#[test]
fn scan_verdicts_survive_journal_roundtrip() {
    let path = temp_journal("scan");
    let _ = std::fs::remove_file(&path);
    let mut j = Journal::create(&path, "main").expect("create");
    let cfg = j.spawn(NodeId(0), NodeKind::Delta, "配置").expect("spawn");
    let report = scan::run_scan(&[(cfg, "secret = \"sk-1\";")], scan::RULES, &[]);
    for d in report.to_deltas("TRI") {
        j.append_delta(&d).expect("append");
    }
    let saved = j.into_dag().expect("flush");

    let j2 = Journal::open(&path).expect("open");
    assert_eq!(j2.dag(), &saved);
    assert_eq!(j2.dag().merge_gate(cfg), GateDecision::Block);
    let _ = std::fs::remove_file(&path);
}
