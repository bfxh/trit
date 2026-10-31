//! Delta 编解码与追加式日志：恶意字符串往返、重放一致性、损坏检测、apply 校验。

use trit_immune::*;

fn temp_journal(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("trit-immune-{name}-{}.journal", std::process::id()))
}

#[test]
fn codec_roundtrip_nasty_strings() {
    let nasty = [
        "含 空格",
        "制\t表",
        "换\n行",
        "回\r车",
        "反\\斜",
        "引\"号",
        "中文✓",
        "OTHER:看起来像标签",
    ];
    let deltas = vec![
        Delta::Root {
            title: nasty[0].to_string(),
        },
        Delta::Spawn {
            parent: NodeId(0),
            kind: NodeKind::AttackVector,
            title: nasty[1].to_string(),
        },
        Delta::Record {
            id: NodeId(1),
            verdict: Trit::Vulnerable,
            evidence: Some(ExploitEvidence {
                vector: AttackVector::Other(nasty[7].to_string()),
                payload: nasty[2].to_string(),
                target: NodeId(1),
                round: 374,
            }),
            note: nasty[5].to_string(),
        },
        Delta::Quarantine {
            id: NodeId(1),
            note: nasty[3].to_string(),
        },
        Delta::HumanReview {
            id: NodeId(1),
            verdict: Trit::Hardened,
            note: nasty[4].to_string(),
        },
        Delta::CrossLink {
            from: NodeId(1),
            to: NodeId(0),
            pass: Trit::Unknown,
            note: nasty[6].to_string(),
        },
    ];
    for d in &deltas {
        let line = trit_immune::delta::encode(d);
        // 单行承诺：任何字段内容都不会把记录劈成两行
        assert!(!line.contains('\n'), "编码泄漏换行：{line:?}");
        assert_eq!(trit_immune::delta::parse_line(&line).as_ref(), Ok(d));
    }
}

#[test]
fn journal_roundtrip_recovers_state() {
    let path = temp_journal("roundtrip");
    let _ = std::fs::remove_file(&path);

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
    j.cross_link(NodeId(0), atk, Trit::Vulnerable, "主干会话被攻击面引用")
        .expect("link");
    let snapshot = j.dag().clone();
    j.into_dag().expect("flush");

    let mut j2 = Journal::open(&path).expect("open");
    assert_eq!(j2.dag(), &snapshot, "重放状态必须与实时状态一致");
    // 未修复的 -1 仍应堵死合并
    assert_eq!(j2.dag().merge_gate(login), GateDecision::Block);
    j2.record(atk, Trit::Hardened, None, "重放失败").expect("record");
    j2.record(login, Trit::Hardened, None, "复验").expect("record");
    let after = j2.dag().clone();
    j2.into_dag().expect("flush");

    let j3 = Journal::open(&path).expect("reopen");
    assert_eq!(j3.dag(), &after);
    assert_eq!(j3.dag().merge_gate(login), GateDecision::Allow);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn corrupt_journal_reports_line() {
    let path = temp_journal("corrupt");
    let _ = std::fs::remove_file(&path);

    // 坏行：解析错误要指认行号
    std::fs::write(
        &path,
        format!("{}\n0:0:0\tROOT\tmain\nGARBAGE\t1\n", journal::HEADER),
    )
    .unwrap();
    match Journal::open(&path).unwrap_err() {
        OpenError::Parse { line, .. } => assert_eq!(line, 3),
        other => panic!("应为解析错误，实际 {other:?}"),
    }

    // 顺序非法：RECORD 指向不存在的节点，apply 错误也要指认行号
    std::fs::write(
        &path,
        format!(
            "{}\n0:0:0\tROOT\tmain\n0:1:0\tRECORD\t9\t+1\t-\t-\t-\t-\tnote\n",
            journal::HEADER
        ),
    )
    .unwrap();
    match Journal::open(&path).unwrap_err() {
        OpenError::Apply { line, .. } => assert_eq!(line, 3),
        other => panic!("应为 apply 错误，实际 {other:?}"),
    }

    // 头部不对：直接拒绝
    std::fs::write(&path, "not-a-journal\n").unwrap();
    assert!(matches!(
        Journal::open(&path),
        Err(OpenError::Header(_))
    ));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn stamps_are_strictly_increasing_and_reopen_is_causal() {
    let path = temp_journal("hlc");
    let _ = std::fs::remove_file(&path);
    let mut j = Journal::create_as(&path, "main", 7).expect("create");
    let f = j.spawn(NodeId(0), NodeKind::Delta, "功能").expect("spawn");
    j.record(f, Trit::Hardened, None, "通过").expect("record");

    // 原始日志：每行都带 HLC 戳，同写者严格递增
    let raw = std::fs::read_to_string(&path).unwrap();
    let mut stamps = Vec::new();
    for line in raw.lines().skip(1) {
        let ts_field = line.split('\t').next().unwrap();
        stamps.push(ts_field.parse::<Hlc>().expect("每条 Delta 都有 HLC 戳"));
    }
    assert!(stamps.len() >= 3);
    for w in stamps.windows(2) {
        assert!(w[1] > w[0], "同写者戳必须严格递增");
    }
    assert!(stamps.iter().all(|t| t.writer == 7));
    let last_in_file = stamps[stamps.len() - 1];
    j.into_dag().unwrap();

    // reopen 观察全部戳 → 追加的新戳必须因果更晚
    let mut j2 = Journal::open_as(&path, 7).expect("open");
    assert!(j2.clock().last() > last_in_file, "open 观察全部戳");
    j2.record(f, Trit::Hardened, None, "复验").expect("record");
    let raw2 = std::fs::read_to_string(&path).unwrap();
    let new_ts = raw2
        .lines()
        .last()
        .unwrap()
        .split('\t')
        .next()
        .unwrap()
        .parse::<Hlc>()
        .unwrap();
    assert!(new_ts > last_in_file, "reopen 后的新戳必须因果更晚");
    j2.into_dag().unwrap();
    let _ = std::fs::remove_file(&path);
}

#[test]
fn non_monotonic_stamps_are_rejected() {
    let path = temp_journal("hlc-bad");
    let _ = std::fs::remove_file(&path);
    std::fs::write(
        &path,
        format!(
            "{}\n10:0:0\tROOT\tmain\n5:0:0\tSPAWN\t0\tDELTA\tx\n",
            journal::HEADER
        ),
    )
    .unwrap();
    match Journal::open(&path).unwrap_err() {
        OpenError::Parse { line, message } => {
            assert_eq!(line, 3);
            assert!(message.contains("时间戳非递增"), "实际消息：{message}");
        }
        other => panic!("应为时间戳错误，实际 {other:?}"),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn apply_validates_dag_invariants() {
    let mut dag = ImmuneDag::empty();
    assert_eq!(
        dag.apply(&Delta::Spawn {
            parent: NodeId(0),
            kind: NodeKind::Delta,
            title: "x".into(),
        }),
        Err(ApplyError::UnknownParent {
            parent: NodeId(0)
        })
    );
    dag.apply(&Delta::Root {
        title: "main".into(),
    })
    .unwrap();
    assert_eq!(
        dag.apply(&Delta::Root {
            title: "again".into()
        }),
        Err(ApplyError::RootAlreadyExists)
    );
    assert_eq!(
        dag.apply(&Delta::Record {
            id: NodeId(5),
            verdict: Trit::Hardened,
            evidence: None,
            note: "n".into(),
        }),
        Err(ApplyError::UnknownNode { id: NodeId(5) })
    );
}

#[test]
fn live_and_replay_are_identical() {
    // 实时路径：直接调方法
    let mut live = ImmuneDag::root("main");
    let f = live.spawn(NodeId(0), NodeKind::Delta, "登录");
    let a = live.spawn(f, NodeKind::AttackVector, "SQLi");
    live.record(a, Trit::Vulnerable, None, "打穿");
    live.quarantine(f, "测试隔离");
    live.human_review(f, Trit::Hardened, "复核");
    live.cross_link(f, a, Trit::Unknown, "攻击面引用");

    // 重放路径：同一批操作编码成 Delta 再 apply
    let replay = vec![
        Delta::Root {
            title: "main".into(),
        },
        Delta::Spawn {
            parent: NodeId(0),
            kind: NodeKind::Delta,
            title: "登录".into(),
        },
        Delta::Spawn {
            parent: NodeId(1),
            kind: NodeKind::AttackVector,
            title: "SQLi".into(),
        },
        Delta::Record {
            id: NodeId(2),
            verdict: Trit::Vulnerable,
            evidence: None,
            note: "打穿".into(),
        },
        Delta::Quarantine {
            id: NodeId(1),
            note: "测试隔离".into(),
        },
        Delta::HumanReview {
            id: NodeId(1),
            verdict: Trit::Hardened,
            note: "复核".into(),
        },
        Delta::CrossLink {
            from: NodeId(1),
            to: NodeId(2),
            pass: Trit::Unknown,
            note: "攻击面引用".into(),
        },
    ];
    let mut replayed = ImmuneDag::empty();
    for d in &replay {
        replayed.apply(d).unwrap();
    }
    assert_eq!(live, replayed, "实时写入与日志重放必须殊途同归");
}
