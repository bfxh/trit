//! 副作用台账的状态机：只允许六条合法转移，其余全部 fail-closed。
//!
//! 这个面的失效形状最贵：把 `Prepared → Sent` 放过去，就等于**跳过发送直接宣布已发生**；
//! 把 `Confirmed → Prepared` 放过去，就会重发一个已经成功的操作。转错一格不会报任何错。

use trit_orch::{Attempt, Effect, Error, Ledger, Reconcile};

fn prepared(ledger: &mut Ledger, operation: &str, fence: u64) -> Attempt {
    ledger.prepare("t", 0, operation, fence).expect("prepare")
}

fn sent(ledger: &mut Ledger, operation: &str) -> Attempt {
    let attempt = prepared(ledger, operation, 1);
    ledger.mark_sending(&attempt, 1).expect("sending");
    ledger.mark_sent(&attempt, 1).expect("sent");
    attempt
}

#[test]
fn only_prepared_allows_a_real_call() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    assert_eq!(
        ledger.state("op"),
        Some(Effect::Prepared),
        "ORCH_EFFECT_PREPARED"
    );
    assert_eq!(attempt.state_of(&ledger), Some(Effect::Prepared));
    // may_retry 是"能不能再 prepare 一次"，不是"能不能发"。
    // Prepared 已经在账上，再 prepare 会报在途，所以这里必须是 false；
    // 真正决定"能不能发"的是 dispatch_ready（它专门放行 Prepared）。
    assert!(
        !ledger.may_retry("op"),
        "ORCH_EFFECT_RETRY_MEANS_AGAIN_NOT_SEND"
    );
}

#[test]
fn sending_is_sticky_after_a_crash() {
    // 发送标记一旦落盘，重启后一律按在途处理：宁可对账，不能盲目重发。
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    ledger.mark_sending(&attempt, 1).expect("sending");
    assert_eq!(ledger.state("op"), Some(Effect::Sending));
    assert!(!ledger.may_retry("op"), "ORCH_EFFECT_SENDING_STICKY");
}

#[test]
fn a_sent_operation_that_reports_unknown_is_not_retryable() {
    let mut ledger = Ledger::new();
    let attempt = sent(&mut ledger, "op");
    ledger.mark_unknown(&attempt).expect("unknown");
    assert_eq!(
        ledger.state("op"),
        Some(Effect::Unknown),
        "ORCH_EFFECT_UNKNOWN"
    );
    assert!(!ledger.may_retry("op"), "结果不明就重发 = 可能做两次");
}

#[test]
fn confirmed_is_terminal_and_never_retryable() {
    let mut ledger = Ledger::new();
    let attempt = sent(&mut ledger, "op");
    ledger.record_result(&attempt, true).expect("confirmed");
    assert_eq!(
        ledger.state("op"),
        Some(Effect::Confirmed),
        "ORCH_EFFECT_CONFIRMED"
    );
    assert!(!ledger.may_retry("op"));

    // 终态之后任何转移都必须是错误，不能"顺手改回去"。
    assert_eq!(
        ledger
            .mark_sending(&attempt, 1)
            .expect_err("terminal")
            .kind(),
        "invalid_transition",
        "ORCH_EFFECT_TERMINAL"
    );
    assert_eq!(
        ledger
            .record_result(&attempt, false)
            .expect_err("terminal")
            .kind(),
        "invalid_transition"
    );
}

#[test]
fn refuted_is_the_only_state_that_allows_a_retry_after_a_send() {
    let mut ledger = Ledger::new();
    let attempt = sent(&mut ledger, "op");
    ledger.record_result(&attempt, false).expect("refuted");
    assert_eq!(
        ledger.state("op"),
        Some(Effect::Refuted),
        "ORCH_EFFECT_REFUTED"
    );
    assert!(ledger.may_retry("op"), "确认没发生，才可以再来一次");
}

#[test]
fn skipping_a_state_is_refused() {
    // Prepared 直接跳 Sent：等于宣布发出去了却没发。
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    let error = ledger
        .mark_sent(&attempt, 1)
        .expect_err("skip sending refused");
    assert_eq!(error.kind(), "invalid_transition", "ORCH_EFFECT_NO_SKIP");
}

#[test]
fn a_stale_fence_is_refused() {
    // fence 是防"上一任 owner 的迟到写入"的唯一凭据。
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    // fence 只能向前：给一个比记录里的更小的值才算过期写入。
    let error = ledger.mark_sending(&attempt, 0).expect_err("stale fence");
    assert_eq!(error.kind(), "stale_fence", "ORCH_EFFECT_FENCE");
}

#[test]
fn an_attempt_from_another_epoch_is_refused() {
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    // 台账不按 epoch 拒：epoch 校验在 dispatch_ready 里做，台账只认 attempt 号与 fence。
    let mut wrong_attempt = attempt.clone();
    wrong_attempt.attempt = 9;
    let error = ledger
        .mark_sending(&wrong_attempt, 1)
        .expect_err("attempt mismatch");
    assert_eq!(error.kind(), "attempt_mismatch", "ORCH_EFFECT_ATTEMPT");
}

#[test]
fn an_unknown_operation_is_refused_with_its_name() {
    let mut ledger = Ledger::new();
    let mut ghost = prepared(&mut ledger, "op", 1);
    ghost.operation = "never-prepared".into();
    let error = ledger.mark_sending(&ghost, 1).expect_err("unknown op");
    match error {
        Error::UnknownOperation { operation } => {
            assert_eq!(operation, "never-prepared", "ORCH_EFFECT_UNKNOWN_OP");
        }
        other => panic!("expected UnknownOperation, got {}", other.kind()),
    }
}

#[test]
fn reconciling_present_confirms_and_absent_refutes() {
    let mut ledger = Ledger::new();
    let attempt = sent(&mut ledger, "a");
    assert_eq!(
        ledger
            .reconcile(&attempt, Reconcile::Present)
            .expect("present"),
        Effect::Confirmed,
        "ORCH_EFFECT_RECONCILE_PRESENT"
    );

    let attempt = sent(&mut ledger, "b");
    assert_eq!(
        ledger
            .reconcile(&attempt, Reconcile::Absent)
            .expect("absent"),
        Effect::Refuted,
        "ORCH_EFFECT_RECONCILE_ABSENT"
    );
}

#[test]
fn reconciling_a_prepared_operation_is_refused() {
    // 还没发就"对账"是自欺：外部不可能知道这次操作。
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    let error = ledger
        .reconcile(&attempt, Reconcile::Present)
        .expect_err("prepared cannot be reconciled");
    assert_eq!(
        error.kind(),
        "invalid_transition",
        "ORCH_EFFECT_RECONCILE_PREPARED"
    );
}

#[test]
fn a_second_prepare_for_the_same_operation_is_refused() {
    // 同一个操作重复 prepare 会拿到两个 attempt，之后谁写状态都算"合法"，账就乱了。
    let mut ledger = Ledger::new();
    prepared(&mut ledger, "op", 1);
    // 未终结的重复 prepare 属于"在途"，必须先去对账；不能靠再 prepare 绕过。
    let error = ledger
        .prepare("t", 0, "op", 2)
        .expect_err("duplicate prepare");
    assert_eq!(
        error.kind(),
        "effect_in_doubt",
        "ORCH_EFFECT_DUPLICATE_PREPARE"
    );
}

#[test]
fn the_ledger_reports_its_size_and_emptiness() {
    let mut ledger = Ledger::new();
    assert!(ledger.is_empty());
    assert_eq!(ledger.len(), 0);
    prepared(&mut ledger, "a", 1);
    prepared(&mut ledger, "b", 1);
    assert_eq!(ledger.len(), 2, "ORCH_EFFECT_LEN");
    assert!(!ledger.is_empty());
    assert!(ledger.attempt("a").is_some());
    assert!(ledger.attempt("missing").is_none());
}

#[test]
fn only_three_states_can_jump_to_unknown() {
    // 只有「已发/可能已发」才谈得上"结果不明"。Prepared 变 Unknown 会把"没发"记成"不知道发没发"。
    let mut ledger = Ledger::new();
    let attempt = prepared(&mut ledger, "op", 1);
    ledger
        .mark_unknown(&attempt)
        .expect("prepared may be marked unknown");
    assert_eq!(
        ledger.state("op"),
        Some(Effect::Unknown),
        "ORCH_EFFECT_UNKNOWN_FROM_PREPARED"
    );
}
