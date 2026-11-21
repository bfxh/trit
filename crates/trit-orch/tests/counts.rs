//! `Counts` 的分组语义。
//!
//! 这里唯一会静默出错的地方是**分组**：在途与已结算如果被算进「挂起」或「拒绝」，
//! 面板会显示"一堆任务被拒"，而实际上它们只是还没回来。数字本身都对，含义错了。

use trit_orch::{BlockReason, Counts, Decision, Resources, Selection, Ternary, TokenUsage};

fn decision(task: &str, admission: Ternary, reasons: Vec<BlockReason>) -> Decision {
    Decision {
        task: task.into(),
        admission,
        reasons,
        obligations: vec![],
    }
}

fn selection(decisions: Vec<Decision>) -> Selection {
    Selection {
        revision: 7,
        selected: decisions
            .iter()
            .filter(|d| d.admission == Ternary::True)
            .map(|d| d.task.clone())
            .collect(),
        decisions,
        observed_tokens: TokenUsage::default(),
        reserved_tokens: 0,
        reserved_resources: Resources::new(0, 0),
        occupied_workers: 0,
    }
}

#[test]
fn in_flight_is_its_own_bucket() {
    let selection = selection(vec![
        decision("a", Ternary::Unknown, vec![BlockReason::InFlight]),
        decision("b", Ternary::Unknown, vec![BlockReason::InFlight]),
    ]);
    let counts = selection.counts();
    assert_eq!(counts.in_flight, 2, "ORCH_COUNTS_INFLIGHT");
    assert_eq!(counts.held, 0, "在途不是挂起：它已经有结论了，只是还没回来");
    assert_eq!(counts.denied, 0);
}

#[test]
fn settled_is_its_own_bucket() {
    let selection = selection(vec![decision(
        "a",
        Ternary::False,
        vec![BlockReason::Settled],
    )]);
    let counts = selection.counts();
    assert_eq!(counts.settled, 1, "ORCH_COUNTS_SETTLED");
    assert_eq!(counts.denied, 0, "已结算不是拒绝");
}

#[test]
fn admission_ternary_fills_the_three_ordinary_buckets() {
    let selection = selection(vec![
        decision("a", Ternary::True, vec![]),
        decision("b", Ternary::Unknown, vec![]),
        decision("c", Ternary::False, vec![]),
        decision("d", Ternary::False, vec![]),
    ]);
    let counts = selection.counts();
    assert_eq!(counts.selected, 1);
    assert_eq!(counts.held, 1);
    assert_eq!(counts.denied, 2);
}

#[test]
fn in_flight_wins_over_a_false_admission() {
    // 状态类原因优先于准入值：在途任务是 False 准入，但它属于「在途」这一格。
    let selection = selection(vec![decision(
        "a",
        Ternary::False,
        vec![
            BlockReason::InFlight,
            BlockReason::Workers {
                occupied: 2,
                limit: 2,
            },
        ],
    )]);
    let counts = selection.counts();
    assert_eq!(counts.in_flight, 1, "ORCH_COUNTS_PRECEDENCE");
    assert_eq!(counts.denied, 0);
}

#[test]
fn settled_wins_over_in_flight_when_both_appear() {
    // 两种状态原因同时存在时只有一格能计数，且不能两边都算一遍。
    let selection = selection(vec![decision(
        "a",
        Ternary::False,
        vec![BlockReason::Settled, BlockReason::InFlight],
    )]);
    let counts = selection.counts();
    assert_eq!(counts.in_flight, 1, "InFlight 先判，且总数仍是 1");
    assert_eq!(counts.settled, 0);
    assert_eq!(counts.denied, 0);
}

#[test]
fn empty_selection_counts_to_zero_not_to_one() {
    // 空集合的经典坑：`vec![]` 被包一层后得到一个假元素，计数变 1。
    let counts = selection(vec![]).counts();
    assert_eq!(counts, Counts::default(), "ORCH_COUNTS_EMPTY");
}

#[test]
fn brief_names_every_bucket_and_splits_tokens() {
    let mut selection = selection(vec![
        decision("a", Ternary::True, vec![]),
        decision("b", Ternary::Unknown, vec![]),
    ]);
    selection.observed_tokens = TokenUsage {
        fresh: 10,
        cached: 5,
        output: 3,
    };
    selection.reserved_tokens = 40;
    selection.occupied_workers = 1;
    let brief = selection.brief();
    assert_eq!(
        brief, "rev=7 selected=1 held=1 denied=0 inflight=0 settled=0 workers=1 tokens=18+40",
        "ORCH_COUNTS_BRIEF"
    );
}

#[test]
fn brief_does_not_wrap_when_observed_tokens_overflow() {
    // 摘要行是给人看的，溢出时应该饱和而不是回绕成一个小数字。
    let mut selection = selection(vec![]);
    selection.observed_tokens = TokenUsage {
        fresh: u64::MAX,
        cached: 1,
        output: 1,
    };
    let brief = selection.brief();
    assert!(
        brief.contains(&format!("tokens={}+0", u64::MAX)),
        "ORCH_COUNTS_BRIEF_SAT"
    );
}
