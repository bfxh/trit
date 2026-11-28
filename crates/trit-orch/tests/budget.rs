//! 资源与 token 预算的守卫测试。
//!
//! 这一面的失效形状是「不报错、只是多花了」：超支、超并发、超容量都不会让任何断言变红，
//! 只会让后面的批次悄悄变慢或把额度吃光。所以这里逐条钉住四类上限的**命中**与**不命中**。

use std::collections::BTreeMap;

use trit_orch::{
    BlockReason, Error, Limits, Observation, Phase, Plan, Resources, Selection, Snapshot, Task,
    TaskSpec, Ternary, TokenUsage,
};

fn resources(cpu: u32, memory: u32) -> Resources {
    Resources::new(cpu, memory)
}

fn spec(id: &str, cpu: u32, memory: u32, max_tokens: u64, duration: u32) -> TaskSpec {
    TaskSpec {
        task: Task {
            id: id.into(),
            priority: 0,
            duration,
            deps: vec![],
        },
        requirements: resources(cpu, memory),
        max_tokens,
        policy: trit_orch::TaskPolicy::default(),
    }
}

fn limits(workers: usize, cpu: u32, memory: u32, tokens: u64) -> Limits {
    Limits {
        workers,
        capacity: resources(cpu, memory),
        tokens,
    }
}

fn observation(phase: Phase, usage: TokenUsage) -> Observation {
    Observation {
        phase,
        authorization: Ternary::True,
        freshness: Ternary::True,
        usage,
        ..Observation::default()
    }
}

fn snapshot(entries: &[(&str, Observation)]) -> Snapshot {
    Snapshot {
        revision: 1,
        observations: entries
            .iter()
            .map(|(id, obs)| ((*id).to_string(), obs.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

fn select(tasks: Vec<TaskSpec>, snapshot: Snapshot, limits: Limits) -> Result<Selection, Error> {
    Plan::new(1, tasks)?.select(&snapshot, &limits)
}

#[test]
fn settled_overrun_is_a_sunk_cost_not_a_permanent_block() {
    // 已经花超了是既成事实。它不能把整张计划永久卡死，只能让这个任务剩余额度归零。
    let task = spec("t", 1, 1, 100, 1);
    let snap = snapshot(&[(
        "t",
        observation(
            Phase::Settled,
            TokenUsage {
                fresh: 150,
                ..TokenUsage::default()
            },
        ),
    )]);
    let selection = select(vec![task], snap, limits(4, 4, 4, 10_000)).expect("settled overrun ok");
    let decision = &selection.decisions[0];
    // 状态类原因不通过 admission 表达：已结算/在途决定的 admission 仍是默认 Unknown。
    // 读 admission 会看到 Unknown，误以为"待补证"——所以判状态必须看 reasons。
    assert_eq!(decision.admission, Ternary::Unknown);
    assert!(
        decision
            .reasons
            .iter()
            .any(|reason| matches!(reason, BlockReason::Settled)),
        "ORCH_BUDGET_SETTLED"
    );
    assert!(decision.obligations.is_empty(), "已结算不该再欠补证动作");
}

#[test]
fn in_flight_overrun_is_refused_instead_of_being_rounded_down() {
    // 在途任务超支说明 max_tokens 这个契约已经破了，不能像 settled 那样当成历史。
    let task = spec("t", 1, 1, 100, 1);
    let snap = snapshot(&[(
        "t",
        observation(
            Phase::InFlight,
            TokenUsage {
                fresh: 150,
                ..TokenUsage::default()
            },
        ),
    )]);
    let error = select(vec![task], snap, limits(4, 4, 4, 10_000)).expect_err("must refuse");
    assert_eq!(
        error.kind(),
        "task_token_limit_exceeded",
        "ORCH_BUDGET_TASK_LIMIT"
    );
}

#[test]
fn exact_task_budget_is_not_an_overrun() {
    // 边界等号：花得正好等于上限是允许的，off-by-one 不能变成拒绝。
    let task = spec("t", 1, 1, 100, 1);
    let snap = snapshot(&[(
        "t",
        observation(
            Phase::InFlight,
            TokenUsage {
                fresh: 100,
                ..TokenUsage::default()
            },
        ),
    )]);
    select(vec![task], snap, limits(4, 4, 4, 10_000)).expect("exact budget allowed");
}

#[test]
fn observed_and_reserved_are_not_double_counted() {
    // 同一个 token 既算「已花」又按整额预留，会让可用额度凭空少一半。
    let task = spec("t", 1, 1, 100, 1);
    let snap = snapshot(&[(
        "t",
        observation(
            Phase::InFlight,
            TokenUsage {
                fresh: 60,
                ..TokenUsage::default()
            },
        ),
    )]);
    // 上限 100，已花 60，预留应为 40。可用 = 100 - 60 - 40 = 0，恰好不越界。
    let selection = select(vec![task], snap, limits(4, 4, 4, 100)).expect("no double count");
    assert_eq!(selection.reserved_tokens, 40, "ORCH_BUDGET_NO_DOUBLE_COUNT");
    assert_eq!(selection.observed_tokens.fresh, 60);
}

#[test]
fn worker_limit_is_refused_with_the_numbers_that_were_compared() {
    let tasks = vec![spec("a", 1, 1, 10, 1), spec("b", 1, 1, 10, 1)];
    let snap = snapshot(&[
        (
            "a",
            observation(
                Phase::InFlight,
                TokenUsage {
                    fresh: 1,
                    ..TokenUsage::default()
                },
            ),
        ),
        (
            "b",
            observation(
                Phase::InFlight,
                TokenUsage {
                    fresh: 1,
                    ..TokenUsage::default()
                },
            ),
        ),
    ]);
    let error = select(tasks, snap, limits(1, 8, 8, 10_000)).expect_err("workers exceeded");
    match error {
        Error::WorkerLimitExceeded { occupied, workers } => {
            assert_eq!((occupied, workers), (2, 1), "ORCH_BUDGET_WORKERS");
        }
        other => panic!("expected WorkerLimitExceeded, got {}", other.kind()),
    }
}

#[test]
fn resource_capacity_overrun_is_denied_without_failing_the_whole_plan() {
    // 单个任务的需求超过整机容量：这不是"等一会儿"，是永远排不上。
    // 但它只能让**这个任务**被拒，不能把整张计划炸掉——否则一个写错的需求会拖住所有任务。
    let task = spec("t", 9, 1, 10, 1);
    let snap = snapshot(&[("t", observation(Phase::Pending, TokenUsage::default()))]);
    let selection = select(vec![task], snap, limits(4, 4, 4, 10_000)).expect("plan survives");
    let decision = &selection.decisions[0];
    assert_eq!(decision.admission, Ternary::False, "ORCH_BUDGET_RESOURCE");
    assert!(
        decision
            .reasons
            .iter()
            .any(|reason| matches!(reason, BlockReason::ResourceCapacity { .. }))
    );
    assert!(selection.selected.is_empty());
}

#[test]
fn total_token_limit_is_refused_with_the_sum_that_was_compared() {
    let tasks = vec![spec("a", 1, 1, 300, 1)];
    let snap = snapshot(&[(
        "a",
        observation(
            Phase::InFlight,
            TokenUsage {
                fresh: 150,
                ..TokenUsage::default()
            },
        ),
    )]);
    let error = select(tasks, snap, limits(4, 4, 4, 100)).expect_err("token limit exceeded");
    match error {
        Error::TokenLimitExceeded {
            observed,
            reserved,
            limit,
        } => {
            assert_eq!(
                (observed, reserved, limit),
                (150, 150, 100),
                "ORCH_BUDGET_TOTAL"
            );
        }
        other => panic!("expected TokenLimitExceeded, got {}", other.kind()),
    }
}

#[test]
fn token_usage_saturates_into_an_error_instead_of_wrapping() {
    // u64 溢出若回绕，就会把"花了很多"读成"花得很少"，是所有成本统计的静默失效源。
    let usage = TokenUsage {
        fresh: u64::MAX,
        cached: 1,
        output: 0,
    };
    let error = usage.total().expect_err("overflow must be an error");
    assert_eq!(error.kind(), "token_overflow", "ORCH_BUDGET_OVERFLOW");
}

#[test]
fn token_overflow_is_checked_across_all_three_fields() {
    for usage in [
        TokenUsage {
            fresh: u64::MAX,
            cached: 1,
            output: 0,
        },
        TokenUsage {
            fresh: 1,
            cached: u64::MAX,
            output: 1,
        },
        TokenUsage {
            fresh: u64::MAX,
            cached: u64::MAX,
            output: u64::MAX,
        },
    ] {
        assert_eq!(
            usage.total().expect_err("must overflow").kind(),
            "token_overflow",
            "ORCH_BUDGET_OVERFLOW_FIELDS"
        );
    }
}

#[test]
fn blocked_reasons_carry_the_three_valued_gate() {
    // 容量不足有两种：暂时不够（Unknown，等）和结构性不够（False，别等）。
    let big = spec("big", 1, 8, 10, 1); // 需要 8 内存，容量 4
    let tasks = vec![big.clone()];
    let snap = snapshot(&[("big", observation(Phase::Pending, TokenUsage::default()))]);
    let selection = select(tasks, snap.clone(), limits(4, 4, 4, 10_000)).expect("gate runs");
    let decision = &selection.decisions[0];
    assert_eq!(decision.admission, Ternary::False, "结构性不足必须是 False");
    assert!(
        decision
            .reasons
            .iter()
            .any(|reason| matches!(reason, BlockReason::ResourceCapacity { .. }))
    );

    // 同样的任务放进容量足够的机器但并发已满：只能是 Unknown，不许升级成确定。
    let tasks = vec![spec("x", 1, 1, 10, 1), spec("y", 1, 1, 10, 1)];
    let snap2 = snapshot(&[
        (
            "x",
            observation(
                Phase::InFlight,
                TokenUsage {
                    fresh: 1,
                    ..TokenUsage::default()
                },
            ),
        ),
        ("y", observation(Phase::Pending, TokenUsage::default())),
    ]);
    let selection2 = select(tasks, snap2, limits(1, 4, 4, 10_000)).expect("gate runs");
    // decisions 是按任务顺序排的：索引 0 是在途的 x，被 worker 卡住的是索引 1 的 y。
    let decision2 = &selection2.decisions[1];
    assert_eq!(
        decision2.admission,
        Ternary::Unknown,
        "ORCH_BUDGET_UNKNOWN_GATE"
    );
    assert!(
        decision2
            .reasons
            .iter()
            .any(|reason| matches!(reason, BlockReason::Workers { .. }))
    );
}

#[test]
fn an_saturated_plan_reports_no_free_token_room() {
    let task = spec("t", 1, 1, 100, 1);
    let snap = snapshot(&[(
        "t",
        observation(
            Phase::Settled,
            TokenUsage {
                fresh: 100,
                ..TokenUsage::default()
            },
        ),
    )]);
    let selection = select(vec![task], snap, limits(4, 4, 4, 100)).expect("exactly saturated");
    assert_eq!(selection.reserved_tokens, 0, "ORCH_BUDGET_SATURATED");
    assert_eq!(selection.occupied_workers, 0, "已结算不该继续占 worker");
}

#[test]
fn pending_tasks_reserve_their_full_budget() {
    // 未开始的任务要按整额预留：否则同一轮里两个任务会共用一份额度。
    let task = spec("t", 2, 1, 50, 1);
    let snap = snapshot(&[("t", observation(Phase::Pending, TokenUsage::default()))]);
    let selection = select(vec![task], snap, limits(4, 8, 8, 10_000)).expect("pending");
    assert_eq!(selection.reserved_tokens, 50, "ORCH_BUDGET_PENDING_RESERVE");
    assert_eq!(selection.occupied_workers, 1);
    assert_eq!(selection.reserved_resources, resources(2, 1));
}
