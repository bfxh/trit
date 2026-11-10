//! 快照：逐字段精确往返（含审计历史）、损坏回退、尾部增量重放、快照路径≈全量重放。

use trit_immune::*;

fn temp_journal(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

fn sidecar(journal: &std::path::Path) -> std::path::PathBuf {
    let mut p = journal.to_path_buf();
    p.set_extension("snapshot");
    p
}

#[test]
fn snapshot_roundtrip_preserves_exact_state() {
    let path = temp_journal("snap");
    let snap_path = sidecar(&path);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&snap_path);

    let mut j = Journal::create(&path, "main").expect("create");
    let login = j.spawn(NodeId(0), NodeKind::Delta, "登录").expect("spawn");
    let atk = j
        .spawn(login, NodeKind::AttackVector, "SQLi")
        .expect("spawn");
    j.record(
        atk,
        Trit::Vulnerable,
        Some(ExploitEvidence {
            vector: AttackVector::SqlInjection,
            payload: "多行\npayload\t带制表".into(),
            target: login,
            round: 2,
        }),
        "打穿",
    )
    .expect("record");
    let pay = j.spawn(NodeId(0), NodeKind::Delta, "支付").expect("spawn");
    j.record(pay, Trit::Hardened, None, "自身通过")
        .expect("record");
    j.cross_link(login, pay, Trit::Vulnerable, "支付复用登录会话解析")
        .expect("link");
    let docs = j.spawn(NodeId(0), NodeKind::Delta, "文档").expect("spawn");
    let page = j.spawn(docs, NodeKind::Message, "外部网页").expect("spawn");
    j.quarantine(page, "注入模式").expect("quarantine");
    j.human_review(page, Trit::Hardened, "复核无注入")
        .expect("review");

    j.snapshot().expect("写快照");
    let saved = j.into_dag().expect("flush");

    let mut j2 = Journal::open(&path).expect("open（应走快照）");
    assert!(j2.recovered_from_snapshot(), "应从快照恢复");
    assert_eq!(j2.tail_replayed(), 0);
    assert_eq!(
        j2.dag(),
        &saved,
        "快照恢复的状态必须逐字段一致（含审计历史、隔离标记、跨树边）"
    );

    // 快照之后追加 → 下次打开只重放尾部增量
    j2.record(atk, Trit::Hardened, None, "重放失败")
        .expect("record");
    j2.record(login, Trit::Hardened, None, "补丁复验")
        .expect("record");
    let after = j2.dag().clone();
    j2.into_dag().expect("flush");

    let j3 = Journal::open(&path).expect("第二次打开");
    assert!(j3.recovered_from_snapshot());
    assert_eq!(j3.tail_replayed(), 2, "快照之后只重放尾部增量");
    assert_eq!(j3.dag(), &after);
    assert_eq!(j3.dag().merge_gate(login), GateDecision::Allow);

    // 快照路径 ≡ 全量重放：删掉快照后再开一次，状态必须殊途同归
    let r1 = j3.into_dag().expect("flush");
    std::fs::remove_file(&snap_path).expect("删快照");
    let j4 = Journal::open(&path).expect("全量重放");
    assert!(!j4.recovered_from_snapshot());
    assert_eq!(j4.dag(), &r1, "快照路径与全量重放殊途同归");

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(sidecar(&path));
}

#[test]
fn corrupt_snapshot_falls_back_to_full_replay() {
    let path = temp_journal("snap-corrupt");
    let snap_path = sidecar(&path);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&snap_path);

    let mut j = Journal::create(&path, "main").expect("create");
    let f = j.spawn(NodeId(0), NodeKind::Delta, "功能").expect("spawn");
    j.record(f, Trit::Hardened, None, "通过").expect("record");
    j.snapshot().expect("写快照");
    let saved = j.into_dag().expect("flush");

    // 破坏快照（结构非法）
    std::fs::write(&snap_path, "trit-immune-snapshot v1\nGARBAGE\n").unwrap();
    let j2 = Journal::open(&path).expect("open");
    assert!(!j2.recovered_from_snapshot(), "损坏的快照应被丢弃");
    assert_eq!(j2.dag(), &saved, "回退全量重放后状态不变");

    // 更狠：快照头部都不对
    std::fs::write(&snap_path, b"\x00\xff\xfegarbage").unwrap();
    let j3 = Journal::open(&path).expect("open");
    assert!(!j3.recovered_from_snapshot());
    assert_eq!(j3.dag(), &saved);

    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(sidecar(&path));
}

#[test]
fn snapshot_parse_rejects_broken_state() {
    // id 不连续
    let bad = "trit-immune-snapshot v1\nCOVERED\t0\t0\tcbf29ce484222325\nNODE\t1\t-\tDELTA\tx\t+1\t0\t0\n";
    assert!(snapshot::parse(bad).is_err());
    // 头部不对
    assert!(snapshot::parse("nope\n").is_err());
    // 空文件
    assert!(snapshot::parse("").is_err());
}
