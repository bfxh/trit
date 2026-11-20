//! 任务图的边界：空 id、重复 id、重复依赖、未知依赖、自依赖、环、任务数上限。
//!
//! 图校验是唯一在构造阶段就能拒绝坏计划的地方。漏一格的表现是"计划能建起来，
//! 执行时才死锁或死循环"，而且日志上看不出是图的问题。

use trit_orch::{Dependency, Error, MAX_TASKS, Plan, Resources, Task, TaskSpec, Ternary};

fn spec(id: &str) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration: 1,
            deps: vec![],
        },
        requirements: Resources::new(1, 1),
        max_tokens: 10,
        epoch: 0,
    }
}

fn with_deps(id: &str, deps: &[&str]) -> TaskSpec {
    let mut spec = spec(id);
    spec.task.deps = deps
        .iter()
        .map(|dep| Dependency {
            task: (*dep).to_string(),
            required: Ternary::True,
        })
        .collect();
    spec
}

#[test]
fn a_blank_id_is_refused() {
    let error = Plan::new(1, vec![spec("  ")]).expect_err("blank id");
    assert_eq!(error.kind(), "blank_task_id", "ORCH_GRAPH_BLANK");
}

#[test]
fn duplicate_ids_are_refused() {
    let error = Plan::new(1, vec![spec("a"), spec("a")]).expect_err("duplicate");
    assert_eq!(error.kind(), "duplicate_task", "ORCH_GRAPH_DUPLICATE");
}

#[test]
fn a_duplicate_dependency_is_refused_because_it_double_counts() {
    // 同一条边写两遍会让"依赖满足数"的计数错位，环检测与准入都会因此失真。
    let error = Plan::new(1, vec![spec("a"), with_deps("b", &["a", "a"])]).expect_err("dup dep");
    assert_eq!(error.kind(), "duplicate_dependency", "ORCH_GRAPH_DUP_DEP");
}

#[test]
fn an_unknown_dependency_is_refused_with_both_names() {
    let error = Plan::new(1, vec![with_deps("b", &["ghost"])]).expect_err("unknown dep");
    match error {
        Error::UnknownDependency { task, dependency } => {
            assert_eq!(
                (task.as_str(), dependency.as_str()),
                ("b", "ghost"),
                "ORCH_GRAPH_UNKNOWN_DEP"
            );
        }
        other => panic!("expected UnknownDependency, got {}", other.kind()),
    }
}

#[test]
fn a_self_dependency_is_a_cycle() {
    let error = Plan::new(1, vec![with_deps("a", &["a"])]).expect_err("self cycle");
    assert_eq!(error.kind(), "cycle", "ORCH_GRAPH_SELF_CYCLE");
}

#[test]
fn a_two_node_cycle_is_refused_and_names_both_nodes() {
    let error =
        Plan::new(1, vec![with_deps("a", &["b"]), with_deps("b", &["a"])]).expect_err("two cycle");
    match error {
        Error::Cycle { blocked } => {
            assert_eq!(blocked.len(), 2, "ORCH_GRAPH_CYCLE_NODES");
        }
        other => panic!("expected Cycle, got {}", other.kind()),
    }
}

#[test]
fn a_long_chain_is_accepted_and_keeps_order() {
    // 10 级链：环检测若写成递归，长链会栈溢出；这里同时也验证顺序稳定。
    let mut tasks = vec![spec("t0")];
    for index in 1..10 {
        tasks.push(with_deps(
            &format!("t{index}"),
            &[&format!("t{}", index - 1)],
        ));
    }
    let plan = Plan::new(1, tasks).expect("chain ok");
    assert_eq!(plan.tasks().len(), 10, "ORCH_GRAPH_CHAIN");
    assert_eq!(plan.tasks()[0].task.id, "t0", "顺序按输入保持");
}

#[test]
fn more_than_the_task_cap_is_refused() {
    let tasks: Vec<TaskSpec> = (0..=MAX_TASKS).map(|i| spec(&format!("t{i}"))).collect();
    let error = Plan::new(1, tasks).expect_err("too many");
    assert_eq!(error.kind(), "too_many_tasks", "ORCH_GRAPH_MAX");

    let exactly: Vec<TaskSpec> = (0..MAX_TASKS).map(|i| spec(&format!("t{i}"))).collect();
    Plan::new(1, exactly).expect("ORCH_GRAPH_MAX_BOUNDARY");
}

#[test]
fn a_diamond_is_not_a_cycle() {
    // a→b, a→c, b→d, c→d：两个前驱汇合到同一个点，容易被朴素 DFS 误判成环。
    let tasks = vec![
        spec("a"),
        with_deps("b", &["a"]),
        with_deps("c", &["a"]),
        with_deps("d", &["b", "c"]),
    ];
    Plan::new(1, tasks).expect("ORCH_GRAPH_DIAMOND");
}

#[test]
fn required_false_edges_still_must_exist() {
    // required=False 的边语义是"这个依赖不需要跑"，但它指向的 ID 仍必须存在，
    // 否则计划里出现一个无法解析的引用，图就是坏的。
    let mut spec = spec("b");
    spec.task.deps = vec![Dependency {
        task: "ghost".into(),
        required: Ternary::False,
    }];
    let error = Plan::new(1, vec![spec]).expect_err("dangling ref");
    assert_eq!(error.kind(), "unknown_dependency", "ORCH_GRAPH_DANGLING");
}

#[test]
fn an_empty_plan_is_valid() {
    let plan = Plan::new(1, vec![]).expect("empty plan");
    assert!(plan.tasks().is_empty());
    assert!(plan.task("anything").is_none());
}

#[test]
fn task_lookup_is_exact_not_prefix() {
    // "a" 与 "ab" 是两个任务。用前缀查找会让补丁改错对象。
    let plan = Plan::new(1, vec![spec("a"), spec("ab")]).expect("build");
    assert_eq!(plan.task("a").map(|s| s.task.id.as_str()), Some("a"));
    assert_eq!(plan.task("ab").map(|s| s.task.id.as_str()), Some("ab"));
    assert!(plan.task("abc").is_none(), "ORCH_GRAPH_LOOKUP");
}
