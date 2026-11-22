//! 轨迹六条不变量的真值表、属性测试与稳定组。
//!
//! 属性测试用手写 SplitMix64 种子化生成器（零外部 crate）：合法轨迹 check 必过、
//! 单点违反注入必被抓、同种子逐字节可复现。种子在测试输出里打印。
//! 存储侧（崩溃恢复/并发追加/一致性/重放确定性）在 `trace_storage_stability.rs`。

use std::time::Instant;

use trit_orch::{
    Actor, LogicalTime, TraceEvent, TraceEventKind, TraceEventStore, TraceInvariant, TraceLog,
    TracePayload, check_trace,
};

#[path = "../common/mod.rs"]
mod common;

use common::{
    expect_single, inject, into_log, serde_json_lines, snapshot, spec, temp_path, valid_events,
};

// ---------------------------------------------------------------------------
// 真值表：六条各自"成立场景 + 违反场景"
// ---------------------------------------------------------------------------

#[test]
fn seq_contiguous_holds_on_dense_and_sparse_but_complete_logs() {
    // 从 1 开始连续 → 干净。
    let log = into_log("t", valid_events(7, "t", 50));
    assert!(check_trace(&log).is_clean());

    // 单事件日志同样干净（没有缺口可报）。
    let mut single = TraceLog::new("t");
    single
        .append(TraceEvent::new(
            "t",
            1,
            TraceEventKind::TaskSelected,
            "c",
            10,
        ))
        .unwrap();
    assert!(check_trace(&single).is_clean());
}

#[test]
fn seq_gap_reports_invariant_position_and_range() {
    // 内部缺口：seq 2 缺 → 报缺口起点 2。
    let mut log = TraceLog::new("t");
    for seq in [1u64, 3, 4] {
        log.append(TraceEvent::new(
            "t",
            seq,
            TraceEventKind::TaskSelected,
            "c",
            10 + seq,
        ))
        .unwrap();
    }
    let violation = expect_single(check_trace(&log), TraceInvariant::SeqContiguous, "内部缺口");
    assert_eq!(violation.seq, Some(2));
    assert_eq!(violation.expected, "seq 2..=2 存在");

    // 开头缺口：seq 1 缺 → gaps 的语义是从 1 起算，不猜末尾。
    let mut log = TraceLog::new("t");
    for seq in [2u64, 3] {
        log.append(TraceEvent::new(
            "t",
            seq,
            TraceEventKind::TaskSelected,
            "c",
            10 + seq,
        ))
        .unwrap();
    }
    let violation = expect_single(check_trace(&log), TraceInvariant::SeqContiguous, "开头缺口");
    assert_eq!(violation.seq, Some(1));
}

#[test]
fn causation_resolvable_holds_for_existing_and_absent_references() {
    let mut log = TraceLog::new("t");
    let first = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 10);
    log.append(first).unwrap();
    // 指向既有事件 → 干净；没有引用 → 也干净。
    let mut second = TraceEvent::new("t", 2, TraceEventKind::EffectSent, "c", 20);
    second.causation_id = Some("t:1".into());
    log.append(second).unwrap();
    assert!(check_trace(&log).is_clean());
}

#[test]
fn dangling_and_self_causation_are_reported() {
    let mut log = TraceLog::new("t");
    log.append(TraceEvent::new(
        "t",
        1,
        TraceEventKind::TaskSelected,
        "c",
        10,
    ))
    .unwrap();
    let mut dangling = TraceEvent::new("t", 2, TraceEventKind::EffectSent, "c", 20);
    dangling.causation_id = Some("t:9".into());
    log.append(dangling).unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::CausationResolvable,
        "悬空",
    );
    assert_eq!(violation.seq, Some(2));
    assert!(violation.actual.contains("t:9"), "{violation:?}");

    let mut log = TraceLog::new("t");
    let mut self_ref = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 10);
    self_ref.causation_id = Some("t:1".into());
    log.append(self_ref).unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::CausationResolvable,
        "自引用",
    );
    assert_eq!(violation.seq, Some(1));
    assert!(violation.actual.contains("自引用"), "{violation:?}");
}

#[test]
fn event_time_not_after_observation_holds_at_boundaries() {
    // event_time < observed_at → 干净；未知（None）→ 干净；相等边界 → 干净。
    let mut log = TraceLog::new("t");
    log.append(TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 100).with_event_time(40))
        .unwrap();
    log.append(TraceEvent::new(
        "t",
        2,
        TraceEventKind::TaskSelected,
        "c",
        200,
    ))
    .unwrap();
    log.append(
        TraceEvent::new("t", 3, TraceEventKind::TaskSelected, "c", 300).with_event_time(300),
    )
    .unwrap();
    assert!(check_trace(&log).is_clean());
}

#[test]
fn event_time_after_observation_is_reported() {
    let mut log = TraceLog::new("t");
    log.append(
        TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 100).with_event_time(101),
    )
    .unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::EventTimeNotAfterObservation,
        "事实晚于获知",
    );
    assert_eq!(violation.seq, Some(1));
    assert!(violation.expected.contains("100") && violation.actual.contains("101"));
}

#[test]
fn recorded_not_before_observation_holds_at_boundaries() {
    // recorded_at == observed_at（默认构造即是）与更大值都干净。
    let mut log = TraceLog::new("t");
    log.append(TraceEvent::new(
        "t",
        1,
        TraceEventKind::TaskSelected,
        "c",
        100,
    ))
    .unwrap();
    let mut later = TraceEvent::new("t", 2, TraceEventKind::TaskSelected, "c", 200);
    later.recorded_at = 250;
    log.append(later).unwrap();
    assert!(check_trace(&log).is_clean());
}

#[test]
fn recorded_before_observation_is_reported() {
    let mut log = TraceLog::new("t");
    let mut event = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 100);
    event.recorded_at = 99;
    log.append(event).unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::RecordedNotBeforeObservation,
        "落盘早于获知",
    );
    assert_eq!(violation.seq, Some(1));
    assert!(violation.actual.contains("99"), "{violation:?}");
}

#[test]
fn hash_chain_holds_for_empty_and_explicit_hashes() {
    // 默认构造（空串哈希）→ 链闭合；显式哈希逐条闭合 → 也干净。
    let mut log = TraceLog::new("t");
    log.append(TraceEvent::new(
        "t",
        1,
        TraceEventKind::TaskSelected,
        "c",
        10,
    ))
    .unwrap();
    log.append(TraceEvent::new(
        "t",
        2,
        TraceEventKind::TaskSelected,
        "c",
        20,
    ))
    .unwrap();

    let mut explicit = TraceLog::new("t2");
    let mut first = TraceEvent::new("t2", 1, TraceEventKind::TaskSelected, "c", 10);
    first.chain_seq = 1;
    first.seal("");
    let mut second = TraceEvent::new("t2", 2, TraceEventKind::TaskSelected, "c", 20);
    second.chain_seq = 2;
    second.seal(&first.payload_hash);
    explicit.append(first).unwrap();
    explicit.append(second).unwrap();
    assert!(check_trace(&explicit).is_clean());
}

#[test]
fn genesis_hash_and_broken_link_are_reported() {
    // 创世事件带非空 prev_hash：没有前驱却声称继承。
    let mut log = TraceLog::new("t");
    let mut genesis = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", 10);
    genesis.prev_hash = "from-old-chain".into();
    log.append(genesis).unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::HashChainLinked,
        "创世非空",
    );
    assert_eq!(violation.seq, Some(1));

    // 中段断链：seq 2 的 prev_hash 对不上 seq 1 的 payload_hash，报在 seq 2。
    let mut log = TraceLog::new("t2");
    log.append(TraceEvent::new(
        "t2",
        1,
        TraceEventKind::TaskSelected,
        "c",
        10,
    ))
    .unwrap();
    let mut broken = TraceEvent::new("t2", 2, TraceEventKind::TaskSelected, "c", 20);
    broken.prev_hash = "tampered".into();
    log.append(broken).unwrap();
    let violation = expect_single(check_trace(&log), TraceInvariant::HashChainLinked, "断链");
    assert_eq!(violation.seq, Some(2));
    assert!(violation.actual.contains("tampered"), "{violation:?}");
}

#[test]
fn fold_revision_holds_through_plan_updates() {
    // 多次 plan 更新后，最后一次 snapshot 对齐最后一次 plan → 干净；
    // 没有 plan 或没有 snapshot 的部分日志 → 不适用，也算干净。
    let mut log = TraceLog::new("t");
    log.append(
        TraceEvent::new("t", 1, TraceEventKind::PlanPatched, "c", 10).with_payload(
            TracePayload::Plan {
                revision: 3,
                tasks: vec![spec("a")],
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t", 2, TraceEventKind::SnapshotCaptured, "c", 20).with_payload(
            TracePayload::Snapshot {
                snapshot: snapshot(3, &["a"]),
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t", 3, TraceEventKind::PlanPatched, "c", 30).with_payload(
            TracePayload::Plan {
                revision: 7,
                tasks: vec![spec("a"), spec("b")],
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t", 4, TraceEventKind::SnapshotCaptured, "c", 40).with_payload(
            TracePayload::Snapshot {
                snapshot: snapshot(7, &["a", "b"]),
            },
        ),
    )
    .unwrap();
    assert!(check_trace(&log).is_clean());

    let mut plan_only = TraceLog::new("t2");
    plan_only
        .append(
            TraceEvent::new("t2", 1, TraceEventKind::PlanPatched, "c", 10).with_payload(
                TracePayload::Plan {
                    revision: 3,
                    tasks: vec![spec("a")],
                },
            ),
        )
        .unwrap();
    assert!(check_trace(&plan_only).is_clean(), "只有 plan：本条不适用");
}

#[test]
fn fold_revision_mismatch_is_reported_at_last_snapshot() {
    let mut log = TraceLog::new("t");
    log.append(
        TraceEvent::new("t", 1, TraceEventKind::PlanPatched, "c", 10).with_payload(
            TracePayload::Plan {
                revision: 7,
                tasks: vec![spec("a")],
            },
        ),
    )
    .unwrap();
    log.append(
        TraceEvent::new("t", 2, TraceEventKind::SnapshotCaptured, "c", 20).with_payload(
            TracePayload::Snapshot {
                snapshot: snapshot(8, &["a"]),
            },
        ),
    )
    .unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::FoldRevisionAligned,
        "折叠错位",
    );
    assert_eq!(violation.seq, Some(2));
    assert!(violation.expected.contains("7") && violation.actual.contains("8"));
}

// ---------------------------------------------------------------------------
// 报告面：独立报告 + 稳定命名
// ---------------------------------------------------------------------------

#[test]
fn two_violations_are_reported_independently_with_counts() {
    // 一条日志同时违反两条：缺口 + 悬空引用。都要报，counts 分组各一。
    let mut log = TraceLog::new("t");
    log.append(TraceEvent::new(
        "t",
        1,
        TraceEventKind::TaskSelected,
        "c",
        10,
    ))
    .unwrap();
    let mut dangling = TraceEvent::new("t", 3, TraceEventKind::EffectSent, "c", 30);
    dangling.causation_id = Some("t:7".into());
    log.append(dangling).unwrap();

    let report = check_trace(&log);
    assert_eq!(report.len(), 2, "{report:?}");
    let counts = report.counts();
    assert_eq!(counts.get("seq_contiguous"), Some(&1));
    assert_eq!(counts.get("causation_resolvable"), Some(&1));

    // assert_clean 消耗报告：干净则 Ok，否则 Err 原样带回全部证书。
    let brief = report.brief();
    let cloned = report.clone();
    let err = report.assert_clean().expect_err("两条违例必须判红");
    assert_eq!(err, cloned);
    assert!(brief.contains("2 violated"), "{brief}");
}

#[test]
fn clean_report_asserts_ok_and_names_are_stable() {
    let log = into_log("t", valid_events(11, "t", 30));
    let report = check_trace(&log);
    let brief = report.brief();
    assert!(report.assert_clean().is_ok());
    assert_eq!(brief, "trace invariants: clean");

    // 名字是 CI 日志里的稳定字符串；ALL 必须覆盖全部六条、无重名。
    let names: Vec<&str> = TraceInvariant::ALL.iter().map(|inv| inv.as_str()).collect();
    assert_eq!(
        names,
        [
            "seq_contiguous",
            "causation_resolvable",
            "event_time_not_after_observation",
            "recorded_not_before_observation",
            "hash_chain_linked",
            "fold_revision_aligned"
        ]
    );
}

// ---------------------------------------------------------------------------
// 属性测试（种子化生成器）
// ---------------------------------------------------------------------------

#[test]
fn property_valid_random_logs_always_pass() {
    let trajectories = 2_000u64;
    let per_log = 100u64;
    for seed in 1..=trajectories {
        let trace = format!("prop-{seed}");
        let log = into_log(&trace, valid_events(seed, &trace, per_log));
        assert_eq!(log.len(), (per_log + 2) as usize);
        let report = check_trace(&log);
        assert!(
            report.is_clean(),
            "seed={seed} 的合法轨迹被判红：{:?}",
            report.violations
        );
    }
}

#[test]
fn property_single_point_injection_is_always_caught() {
    // 每条不变量 × 200 个种子：注入恰好一处 → 必须恰好报出那一条、那个位置。
    for seed in 1..=200u64 {
        for invariant in TraceInvariant::ALL {
            let trace = format!("inj-{seed}");
            let mut events = valid_events(seed, &trace, 100);
            let injected_seq = inject(&mut events, invariant, seed);
            let violation = expect_single(
                check_trace(&into_log(&trace, events)),
                invariant,
                &format!("seed={seed} invariant={}", invariant.as_str()),
            );
            assert_eq!(
                violation.seq,
                Some(injected_seq),
                "seed={seed} {}: 报错位置 {} 与注入位置 {injected_seq} 不符",
                invariant.as_str(),
                violation.seq.unwrap_or(0)
            );
        }
    }
}

#[test]
fn property_same_seed_is_reproducible_and_different_seed_differs() {
    let seed = 20_261_121;
    println!("property generator seed = {seed}");
    let first = valid_events(seed, "t", 100);
    let second = valid_events(seed, "t", 100);
    assert_eq!(first, second, "同种子必须逐事件复现");
    assert_eq!(
        serde_json_lines(&first),
        serde_json_lines(&second),
        "序列化字节也必须一致"
    );

    let other = valid_events(seed + 1, "t", 100);
    assert_ne!(serde_json_lines(&first), serde_json_lines(&other));
}

#[test]
fn property_valid_logs_survive_store_roundtrip_clean() {
    for seed in 1..=10u64 {
        let trace = format!("rt-{seed}");
        let events = valid_events(seed, &trace, 100);
        let mut sealed = Vec::with_capacity(events.len());
        let mut previous = String::new();
        for (index, event) in events.iter().enumerate() {
            let mut copy = event.clone();
            copy.chain_seq = (index + 1) as u64;
            copy.seal(&previous);
            previous = copy.payload_hash.clone();
            sealed.push(copy);
        }
        let memory = into_log(&trace, sealed);
        let path = temp_path(&format!("roundtrip-{seed}"));
        let store = TraceEventStore::new(&path);
        for event in &events {
            store.append(event).expect("append");
        }
        let loaded = store.load_log(&trace).expect("load_log");
        assert_eq!(loaded, memory, "seed={seed}: 回读的日志必须与内存一致");
        assert!(
            check_trace(&loaded).is_clean(),
            "seed={seed}: 回读后必须仍然干净"
        );
        let _ = std::fs::remove_file(&path);
    }
}

// ---------------------------------------------------------------------------
// 稳定组：1 万级事件与极端边界
// ---------------------------------------------------------------------------

#[test]
fn ten_thousand_event_log_check_is_clean_deterministic_and_bounded() {
    let log = into_log("t", valid_events(99, "t", 10_000));
    assert_eq!(log.len(), 10_002);

    let started = Instant::now();
    let first = check_trace(&log);
    let elapsed = started.elapsed();
    assert!(
        first.is_clean(),
        "10k 合法日志必须干净：{:?}",
        first.violations
    );
    // 有界：调试构建下宽限 10 秒；实现是 O(n log n)，正常在毫秒级。
    assert!(
        elapsed.as_secs() < 10,
        "check 在 10k 事件上耗时 {elapsed:?}，超出有界预期"
    );
    let second = check_trace(&log);
    assert_eq!(first, second, "check 必须是纯函数：两次结果一致");
}

#[test]
fn extreme_values_and_long_names_do_not_break_check() {
    // 空 / 单事件。
    assert!(check_trace(&TraceLog::new("t")).is_clean());

    // LogicalTime 与时间戳全 u64::MAX：相等边界不越线，不 panic。
    let mut log = TraceLog::new("t");
    let mut event = TraceEvent::new("t", 1, TraceEventKind::TaskSelected, "c", u64::MAX)
        .with_logical(LogicalTime {
            revision: u64::MAX,
            epoch: u64::MAX,
            fence: u64::MAX,
            round: u64::MAX,
            attempt: u64::MAX,
        })
        .with_event_time(u64::MAX);
    event.recorded_at = u64::MAX;
    event.actor = Actor::Agent("超长".repeat(5_000));
    log.append(event).unwrap();
    assert!(check_trace(&log).is_clean());

    // u64 边界上只比较、不溢出：event_time 顶到 MAX 而获知只到 MAX-1 → 精确报出。
    let mut log = TraceLog::new("t2");
    log.append(
        TraceEvent::new("t2", 1, TraceEventKind::TaskSelected, "c", u64::MAX - 1)
            .with_event_time(u64::MAX),
    )
    .unwrap();
    let violation = expect_single(
        check_trace(&log),
        TraceInvariant::EventTimeNotAfterObservation,
        "u64 边界",
    );
    assert_eq!(violation.seq, Some(1));

    // seq 在 u64::MAX 的孤立事件：开头缺口 (1..=MAX-1) 必须报出而不 panic。
    let mut log = TraceLog::new("t3");
    log.append(TraceEvent::new(
        "t3",
        u64::MAX,
        TraceEventKind::TaskSelected,
        "c",
        10,
    ))
    .unwrap();
    let violation = expect_single(check_trace(&log), TraceInvariant::SeqContiguous, "极大 seq");
    assert_eq!(violation.seq, Some(1));
    assert!(violation.expected.contains("1..="), "{violation:?}");
}
