//! 第 40 轮 F1 回归：`dispatch_ready` 合并 `selection.selected` 时按任务 id 去重。
//!
//! 修复前：重复 id 原样透传为两条完全相同的 Attempt；修复后：同一 id 只保留**首个**
//! 出现的一次，顺序沿用 `selected` 原序，不再产生重复派发。

use trit_orch::{Ledger, Resources, Selection, Task, TaskSpec, TokenUsage, dispatch_ready};

fn spec(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 100,
        policy: trit_orch::TaskPolicy::default(),
    }
}

fn selection(selected: Vec<&str>) -> Selection {
    Selection {
        revision: 1,
        selected: selected.iter().map(|s| s.to_string()).collect(),
        decisions: vec![],
        observed_tokens: TokenUsage::default(),
        reserved_tokens: 0,
        reserved_resources: Resources::new(0, 0),
        occupied_workers: 0,
    }
}

fn prepared_ledger(ids: &[&str]) -> Ledger {
    let mut ledger = Ledger::new();
    for id in ids {
        ledger.prepare(id, 0, id, 1).unwrap();
    }
    ledger
}

fn ops(ready: &[trit_orch::Attempt]) -> Vec<&str> {
    ready.iter().map(|a| a.operation.as_str()).collect()
}

/// `["a","a"]`：重复条目折叠为一条 Attempt，不重复派发。
#[test]
fn duplicate_adjacent_ids_collapse_to_one() {
    let tasks = vec![spec("a")];
    let ledger = prepared_ledger(&["a"]);
    let ready = dispatch_ready(&selection(vec!["a", "a"]), &tasks, &ledger).unwrap();
    assert_eq!(ops(&ready), vec!["a"], "ORCH_DISPATCH_DEDUP_ADJACENT");
    assert_eq!(ready.len(), 1, "同一任务不得列出两条 Attempt");
}

/// `["a","b","a"]`：保留首次出现顺序 `a, b`，第二个 `a` 丢弃。
#[test]
fn duplicate_in_middle_keeps_first_occurrence_order() {
    let tasks = vec![spec("a"), spec("b")];
    let ledger = prepared_ledger(&["a", "b"]);
    let ready = dispatch_ready(&selection(vec!["a", "b", "a"]), &tasks, &ledger).unwrap();
    assert_eq!(ops(&ready), vec!["a", "b"], "ORCH_DISPATCH_DEDUP_ORDER");
    assert_eq!(ready.len(), 2);
}

/// 空选中列表：ready 为空。
#[test]
fn empty_selection_yields_no_attempts() {
    let tasks = vec![spec("a")];
    let ledger = prepared_ledger(&["a"]);
    let ready = dispatch_ready(&selection(vec![]), &tasks, &ledger).unwrap();
    assert!(ready.is_empty(), "ORCH_DISPATCH_EMPTY");
}

/// 单元素列表：正常派发一次。
#[test]
fn single_id_dispatches_once() {
    let tasks = vec![spec("a")];
    let ledger = prepared_ledger(&["a"]);
    let ready = dispatch_ready(&selection(vec!["a"]), &tasks, &ledger).unwrap();
    assert_eq!(ops(&ready), vec!["a"]);
    assert_eq!(ready.len(), 1);
}

/// 重复的未知 id：首个出现仍走既有校验报错，第二条重复项不掩盖首条错误。
#[test]
fn duplicate_unknown_id_still_reports_first_occurrence() {
    let tasks = vec![spec("a")];
    let ledger = Ledger::new();
    let result = dispatch_ready(&selection(vec!["ghost", "ghost"]), &tasks, &ledger);
    let error = result.expect_err("未知任务必须报错");
    match error {
        trit_orch::Error::NotPrepared { operation, .. } => {
            assert_eq!(operation, "ghost");
        }
        other => panic!("应为 NotPrepared，得到 {other:?}"),
    }
    assert_eq!(ledger.state("ghost"), None);
}
