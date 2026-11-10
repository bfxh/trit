use std::path::PathBuf;

use trit_orch::{
    Attempt, Effect, Error, Ledger, LedgerStore, Limits, Observation, Plan, Reconcile, Resources,
    Snapshot, Task, TaskSpec, Ternary, dispatch_ready,
};

fn task(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        epoch: 0,
    }
}

fn snapshot(tasks: &[TaskSpec]) -> Snapshot {
    Snapshot {
        revision: 1,
        observations: tasks
            .iter()
            .map(|spec| {
                (
                    spec.task.id.clone(),
                    Observation {
                        authorization: Ternary::True,
                        freshness: Ternary::True,
                        ..Observation::default()
                    },
                )
            })
            .collect(),
    }
}

fn limits() -> Limits {
    Limits {
        workers: 2,
        capacity: Resources::new(2, 2),
        tokens: 10_000,
    }
}

fn temp_path(tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "trit-orch-{tag}-{}-{:?}.json",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_file(&path);
    path
}

#[test]
fn ledger_roundtrips_through_disk() {
    let path = temp_path("roundtrip");
    let store = LedgerStore::new(&path);
    assert!(
        store.load().unwrap().is_empty(),
        "missing file is an empty ledger"
    );

    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("task-a", 3, "task-a", 7).unwrap();
    ledger.mark_sending(&attempt, 7).unwrap();
    ledger.mark_unknown(&attempt).unwrap();
    store.save(&ledger).unwrap();

    let loaded = store.load().unwrap();
    assert_eq!(loaded, ledger, "ORCH_LEDGER_ROUNDTRIP");
    assert_eq!(loaded.state("task-a"), Some(Effect::Unknown));
    assert_eq!(loaded.attempt("task-a").unwrap().epoch, 3);
    let _ = std::fs::remove_file(&path);
}

/// 同名中转文件是并发串内容的入口。这里**不用线程**也能判红：
/// 预置一个与旧写法（`path.with_extension("tmp")`）同名的文件，save 之后它必须原样还在。
#[test]
fn save_does_not_consume_a_foreign_tmp_named_file() {
    let path = temp_path("foreign-tmp");
    let store = LedgerStore::new(&path);
    let legacy_tmp = path.with_extension("tmp");
    std::fs::write(&legacy_tmp, b"foreign-tmp-sentinel").unwrap();

    let mut ledger = Ledger::new();
    ledger.prepare("job", 0, "job", 1).unwrap();
    store
        .save(&ledger)
        .unwrap_or_else(|error| panic!("ORCH_LEDGER_TMP_OWNED: 保存不该失败: {error}"));

    assert_eq!(
        std::fs::read(&legacy_tmp).unwrap_or_default(),
        b"foreign-tmp-sentinel",
        "ORCH_LEDGER_TMP_OWNED: save 不该占用/改名掉与目标同前缀的 .tmp 文件"
    );
    assert_eq!(
        store.load().unwrap().attempt("job").unwrap().attempt,
        1,
        "ORCH_LEDGER_TMP_OWNED: 台账本身仍要写成功"
    );
    let _ = std::fs::remove_file(&legacy_tmp);
    let _ = std::fs::remove_file(&path);
}

/// 同一台账文件被两个 store 并发保存：中转名必须每次唯一，否则一方会把另一方的字节
/// rename 进自己的路径，或第二次 rename 因源文件已消失而报错。
#[test]
fn concurrent_saves_on_one_path_never_steal_each_others_bytes() {
    use std::sync::{Arc, Barrier};

    let path = temp_path("same-path");
    let reader = LedgerStore::new(&path);
    let mut big = Ledger::new();
    for name in ["a", "b", "c", "d", "e"] {
        big.prepare(name, 0, name, 1).unwrap();
    }
    let mut small = Ledger::new();
    small.prepare("z", 0, "z", 1).unwrap();

    for round in 0..24 {
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [big.clone(), small.clone()]
            .into_iter()
            .map(|ledger| {
                let barrier = Arc::clone(&barrier);
                let store = LedgerStore::new(path.clone());
                std::thread::spawn(move || {
                    barrier.wait();
                    store.save(&ledger)
                })
            })
            .collect();
        for handle in handles {
            let result = handle
                .join()
                .unwrap_or_else(|_| panic!("ORCH_LEDGER_TMP_UNIQUE: 第 {round} 轮线程 panic"));
            result.unwrap_or_else(|error| {
                panic!("ORCH_LEDGER_TMP_UNIQUE: 第 {round} 轮同路径保存不该失败: {error}")
            });
        }
        let loaded = reader.load().unwrap_or_else(|error| {
            panic!("ORCH_LEDGER_TMP_UNIQUE: 第 {round} 轮载入失败: {error}")
        });
        let present: Vec<&str> = ["a", "b", "c", "d", "e", "z"]
            .iter()
            .filter(|name| loaded.attempt(name).is_some())
            .copied()
            .collect();
        assert!(
            present.len() == 5 || present.len() == 1,
            "ORCH_LEDGER_TMP_UNIQUE: 第 {round} 轮落盘不是任一方的完整台账: {present:?}"
        );
    }

    let stem = path.file_name().unwrap().to_string_lossy().into_owned();
    let stray: Vec<String> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(&stem) && name.ends_with(".tmp"))
        .collect();
    assert!(
        stray.is_empty(),
        "ORCH_LEDGER_TMP_CLEANUP: 保存后不该留下中转文件: {stray:?}"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn corrupt_ledger_is_reported_not_guessed() {
    let path = temp_path("corrupt");
    std::fs::write(&path, "{not json").unwrap();
    let store = LedgerStore::new(&path);
    assert!(
        matches!(store.load(), Err(Error::LedgerParse { .. })),
        "ORCH_LEDGER_PARSE"
    );
    let bad_state = r#"{"records":{"x":{"attempt":{"operation":"x","task":"t","epoch":1,"attempt":1,"fence":1},"state":"Happened"}}}"#;
    assert!(matches!(
        Ledger::from_json(bad_state),
        Err(Error::LedgerParse { .. })
    ));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn save_replaces_previous_contents() {
    let path = temp_path("replace");
    let store = LedgerStore::new(&path);
    let mut first = Ledger::new();
    first.prepare("a", 0, "a", 1).unwrap();
    store.save(&first).unwrap();
    let mut second = Ledger::new();
    second.prepare("b", 0, "b", 1).unwrap();
    store.save(&second).unwrap();
    let loaded = store.load().unwrap();
    assert_eq!(loaded.len(), 1, "ORCH_LEDGER_REPLACE");
    assert_eq!(loaded.state("a"), None);
    assert_eq!(loaded.state("b"), Some(Effect::Prepared));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dispatch_requires_a_prepared_intent_for_every_selected_task() {
    let tasks = vec![task("a"), task("b")];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan.select(&snapshot(&tasks), &limits()).unwrap();
    assert_eq!(selection.selected, vec!["b", "a"], "同优先级按 ID 降序");
    let empty = Ledger::new();
    assert!(matches!(
        dispatch_ready(&selection, &tasks, &empty),
        Err(Error::NotPrepared { .. })
    ));

    let mut partial = Ledger::new();
    partial.prepare("a", 0, "a", 1).unwrap();
    assert!(matches!(
        dispatch_ready(&selection, &tasks, &partial),
        Err(Error::NotPrepared { operation, .. }) if operation == "b"
    ));

    let mut full = Ledger::new();
    full.prepare("a", 0, "a", 1).unwrap();
    full.prepare("b", 0, "b", 1).unwrap();
    let ready = dispatch_ready(&selection, &tasks, &full).unwrap();
    assert_eq!(
        ready
            .iter()
            .map(|a: &Attempt| a.operation.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a"]
    );
}

#[test]
fn dispatch_rejects_intents_prepared_for_an_old_definition_epoch() {
    let tasks = vec![task("a")];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan.select(&snapshot(&tasks), &limits()).unwrap();
    let mut ledger = Ledger::new();
    ledger.prepare("a", 0, "a", 1).unwrap();
    assert!(dispatch_ready(&selection, &tasks, &ledger).is_ok());
    let mut bumped = tasks.clone();
    bumped[0].epoch = 1;
    assert_eq!(
        dispatch_ready(&selection, &bumped, &ledger),
        Err(Error::PreparedEpochMismatch {
            operation: "a".into(),
            prepared: 0,
            expected: 1
        }),
        "ORCH_DISPATCH_EPOCH"
    );
}

#[test]
fn sent_or_confirmed_intents_are_not_dispatchable_again() {
    let tasks = vec![task("a")];
    let plan = Plan::new(1, tasks.clone()).unwrap();
    let selection = plan.select(&snapshot(&tasks), &limits()).unwrap();
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("a", 0, "a", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    ledger.mark_sent(&attempt, 1).unwrap();
    assert!(
        matches!(
            dispatch_ready(&selection, &tasks, &ledger),
            Err(Error::NotPrepared { .. })
        ),
        "ORCH_DISPATCH_STATE"
    );

    let mut done = Ledger::new();
    let attempt = done.prepare("a", 0, "a", 1).unwrap();
    done.mark_sending(&attempt, 1).unwrap();
    done.reconcile(&attempt, Reconcile::Present).unwrap();
    assert!(matches!(
        dispatch_ready(&selection, &tasks, &done),
        Err(Error::NotPrepared { .. })
    ));
}

#[test]
fn replayed_ledger_still_blocks_blind_retry() {
    let path = temp_path("replay");
    let store = LedgerStore::new(&path);
    let mut ledger = Ledger::new();
    let attempt = ledger.prepare("task-a", 0, "task-a", 1).unwrap();
    ledger.mark_sending(&attempt, 1).unwrap();
    ledger.mark_sent(&attempt, 1).unwrap();
    ledger.mark_unknown(&attempt).unwrap();
    store.save(&ledger).unwrap();

    let mut replayed = store.load().unwrap();
    assert!(!replayed.may_retry("task-a"));
    assert_eq!(
        replayed.prepare("task-a", 0, "task-a", 2),
        Err(Error::EffectInDoubt {
            operation: "task-a".into()
        }),
        "ORCH_REPLAY_NO_BLIND_RETRY"
    );
    let stored = replayed.attempt("task-a").unwrap().clone();
    replayed.reconcile(&stored, Reconcile::Absent).unwrap();
    assert!(replayed.may_retry("task-a"));
    let _ = std::fs::remove_file(&path);
}
