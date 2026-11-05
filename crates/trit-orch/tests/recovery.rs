use std::path::PathBuf;

use trit_orch::{Effect, Ledger, LedgerStore, Reconcile, Recovery};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
enum CrashPoint {
    AfterPrepare,
    AfterSendingMark,
    AfterInvoke,
    AfterOutcomeInMemory,
    AfterOutcomeSaved,
}

const ALL_POINTS: [CrashPoint; 5] = [
    CrashPoint::AfterPrepare,
    CrashPoint::AfterSendingMark,
    CrashPoint::AfterInvoke,
    CrashPoint::AfterOutcomeInMemory,
    CrashPoint::AfterOutcomeSaved,
];

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    // 同 tag 会被两个测试同时用到（逐点扫描与单点用例），并行时共用一个文件就会互相删；
    // libtest 保证同时在跑的用例占不同线程，所以带上线程号即可隔离。
    path.push(format!(
        "trit-orch-crash-{tag}-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

/// 在指定边界“崩溃”：只保留已经落盘的台账，然后从磁盘重放。
/// 返回重启后的分类，以及副作用是否真的发生过。
fn crash_run(point: CrashPoint, effect_occurs: bool) -> (Recovery, bool) {
    let path = temp_path(&format!("{point:?}-{effect_occurs}"));
    let store = LedgerStore::new(&path);
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("task-a", 0, "job", 1).unwrap();
    store.save(&ledger).unwrap();

    if point == CrashPoint::AfterPrepare {
        let recovery = store.load().unwrap().recover();
        let _ = std::fs::remove_file(&path);
        return (recovery, false);
    }

    ledger.mark_sending(&attempt, 1).unwrap();
    store.save(&ledger).unwrap();
    if point == CrashPoint::AfterSendingMark {
        let recovery = store.load().unwrap().recover();
        let _ = std::fs::remove_file(&path);
        return (recovery, false);
    }

    // 这里发生外部副作用（由 effect_occurs 模拟）。
    if point == CrashPoint::AfterInvoke {
        let recovery = store.load().unwrap().recover();
        let _ = std::fs::remove_file(&path);
        return (recovery, effect_occurs);
    }

    ledger.mark_sent(&attempt, 1).unwrap();
    ledger.record_result(&attempt, true).unwrap();
    if point == CrashPoint::AfterOutcomeInMemory {
        let recovery = store.load().unwrap().recover();
        let _ = std::fs::remove_file(&path);
        return (recovery, effect_occurs);
    }

    store.save(&ledger).unwrap();
    let recovery = store.load().unwrap().recover();
    let _ = std::fs::remove_file(&path);
    (recovery, effect_occurs)
}

#[test]
fn prepared_is_safe_to_send_after_restart() {
    let (recovery, effect) = crash_run(CrashPoint::AfterPrepare, false);
    assert_eq!(recovery.ready, vec!["job"], "ORCH_CRASH_PREPARED_READY");
    assert!(recovery.in_doubt.is_empty());
    assert!(!effect);
}

#[test]
fn durable_sending_mark_is_in_doubt_even_if_the_call_never_ran() {
    let (recovery, effect) = crash_run(CrashPoint::AfterSendingMark, false);
    assert_eq!(
        recovery.in_doubt,
        vec!["job"],
        "ORCH_CRASH_SENDING_IN_DOUBT"
    );
    assert!(recovery.ready.is_empty());
    assert!(!effect);
}

#[test]
fn crash_after_the_effect_is_never_reported_as_ready() {
    for point in [CrashPoint::AfterInvoke, CrashPoint::AfterOutcomeInMemory] {
        let (recovery, effect) = crash_run(point, true);
        assert!(effect);
        assert!(
            recovery.ready.is_empty(),
            "ORCH_CRASH_NO_DUPLICATE: {point:?} 把可能已生效的操作判成可重发"
        );
        assert_eq!(recovery.in_doubt, vec!["job"], "{point:?}");
    }
}

#[test]
fn lost_in_memory_outcome_reconciles_to_confirmed_without_resending() {
    let path = temp_path("lost-outcome");
    let store = LedgerStore::new(&path);
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("task-a", 0, "job", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    store.save(&ledger).unwrap();
    // 副作用发生、内存里也记了成功，但还没落盘就崩了。
    ledger.mark_sent(&attempt, 1).unwrap();
    ledger.record_result(&attempt, true).unwrap();

    let mut replayed = store.load().unwrap();
    assert_eq!(
        replayed.state("job"),
        Some(Effect::Sending),
        "ORCH_CRASH_DURABLE_STATE"
    );
    let stored = replayed.attempt("job").unwrap().clone();
    assert_eq!(
        replayed.reconcile(&stored, Reconcile::Present).unwrap(),
        Effect::Confirmed
    );
    assert!(replayed.recover().done == vec!["job"]);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn absent_effect_after_crash_becomes_retryable() {
    let (recovery, effect) = crash_run(CrashPoint::AfterInvoke, false);
    assert!(recovery.in_doubt == vec!["job"]);
    assert!(!effect);
    // 对账 Absent 之后才允许重试。
    let path = temp_path("absent");
    let store = LedgerStore::new(&path);
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("task-a", 0, "job", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    store.save(&ledger).unwrap();
    let mut replayed = store.load().unwrap();
    let stored = replayed.attempt("job").unwrap().clone();
    assert!(!replayed.may_retry("job"), "ORCH_CRASH_NO_BLIND_RETRY");
    replayed.reconcile(&stored, Reconcile::Absent).unwrap();
    assert!(
        replayed.may_retry("job"),
        "ORCH_CRASH_RETRY_AFTER_RECONCILE"
    );
    let next = replayed.prepare("task-a", 0, "job", 2).unwrap();
    assert_eq!(next.attempt, 2);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn saved_outcome_survives_restart_as_done() {
    let (recovery, effect) = crash_run(CrashPoint::AfterOutcomeSaved, true);
    assert!(effect);
    assert_eq!(recovery.done, vec!["job"], "ORCH_CRASH_DONE");
    assert!(recovery.ready.is_empty());
    assert!(recovery.in_doubt.is_empty());
}

#[test]
fn every_crash_point_is_conservative_about_possible_effects() {
    for point in ALL_POINTS {
        for effect in [false, true] {
            let (recovery, happened) = crash_run(point, effect);
            if happened {
                assert!(
                    recovery.ready.is_empty(),
                    "ORCH_CRASH_NO_DUPLICATE: {point:?} 效果已发生却仍可派发"
                );
            }
            let total = recovery.ready.len()
                + recovery.in_doubt.len()
                + recovery.retryable.len()
                + recovery.done.len();
            assert_eq!(total, 1, "{point:?} 丢了台账记录");
        }
    }
}
