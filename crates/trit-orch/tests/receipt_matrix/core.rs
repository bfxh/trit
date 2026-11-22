//! 交接回执的状态机：四态只能逐级推进，失败之后任何推进都必须被拒。
//!
//! 回执是"对方到底收没收到"的唯一凭据。跳态（Sent → Acted）会让日志声称"已执行"，
//! 而实际只发出了一个字节流；失败后继续推进则会让一次明确的拒绝被后到的"成功"覆盖。

use trit_orch::{
    DeliveryReceipt, ReceiptFailure, ReceiptLedger, ReceiptState, handoff_gate, handoff_obligation,
};

fn receipt() -> DeliveryReceipt {
    DeliveryReceipt::sent("r1", "key-1").expect("sent")
}

#[test]
fn the_happy_path_is_strictly_monotone() {
    let mut receipt = receipt();
    assert_eq!(receipt.state, ReceiptState::Sent);
    for state in [
        ReceiptState::Delivered,
        ReceiptState::Read,
        ReceiptState::Acted,
    ] {
        receipt.advance(state).expect("forward");
        assert_eq!(receipt.state, state, "ORCH_RECEIPT_FORWARD");
    }
    assert!(receipt.is_acted());
    assert!(!receipt.is_failed());
}

#[test]
fn replaying_the_same_state_is_idempotent_not_an_error() {
    // 网络重试会重复投递同一条回执。同态推进必须无害，否则重试会变成错误。
    let mut receipt = receipt();
    receipt.advance(ReceiptState::Sent).expect("same state ok");
    assert_eq!(receipt.state, ReceiptState::Sent, "ORCH_RECEIPT_IDEMPOTENT");
}

#[test]
fn every_skip_is_refused() {
    // 三对非法跳跃：Sent→Read、Sent→Acted、Delivered→Acted。
    // 每一条都要么被拒、要么（如果起点已经走过那一级）根本不该被用来跳。
    let mut from_sent = receipt();
    assert_eq!(
        from_sent
            .advance(ReceiptState::Read)
            .expect_err("Sent→Read")
            .kind(),
        "receipt_skip",
        "ORCH_RECEIPT_NO_SKIP_DELIVERED"
    );
    assert_eq!(from_sent.state, ReceiptState::Sent, "被拒后状态不变");

    assert_eq!(
        from_sent
            .advance(ReceiptState::Acted)
            .expect_err("Sent→Acted")
            .kind(),
        "receipt_skip",
        "ORCH_RECEIPT_NO_SKIP_ACTED"
    );

    let mut from_delivered = receipt();
    from_delivered
        .advance(ReceiptState::Delivered)
        .expect("walk");
    assert_eq!(
        from_delivered
            .advance(ReceiptState::Acted)
            .expect_err("Delivered→Acted")
            .kind(),
        "receipt_skip",
        "ORCH_RECEIPT_NO_SKIP_DELIVERED_TO_ACTED"
    );
    assert_eq!(from_delivered.state, ReceiptState::Delivered);
}

#[test]
fn backwards_moves_are_refused() {
    let mut receipt = receipt();
    receipt.advance(ReceiptState::Delivered).expect("forward");
    let error = receipt.advance(ReceiptState::Sent).expect_err("backwards");
    assert_eq!(error.kind(), "receipt_skip", "ORCH_RECEIPT_NO_BACKWARDS");
    assert_eq!(receipt.state, ReceiptState::Delivered);
}

#[test]
fn acted_is_terminal() {
    let mut receipt = receipt();
    receipt.advance(ReceiptState::Delivered).expect("d");
    receipt.advance(ReceiptState::Read).expect("r");
    receipt.advance(ReceiptState::Acted).expect("a");
    assert_eq!(ReceiptState::Acted.next(), None, "ORCH_RECEIPT_TERMINAL");
    let error = receipt
        .advance(ReceiptState::Read)
        .expect_err("acted is terminal");
    assert_eq!(error.kind(), "receipt_skip");
}

#[test]
fn a_failure_freezes_the_receipt() {
    // 一旦拒绝/过期，后续任何"成功"推进都不能把这次失败洗掉。
    let mut receipt = receipt();
    receipt.reject("对方拒收").expect("reject");
    assert!(receipt.is_failed());
    let error = receipt
        .advance(ReceiptState::Delivered)
        .expect_err("frozen");
    assert_eq!(error.kind(), "receipt_failed", "ORCH_RECEIPT_FROZEN");
}

#[test]
fn expiring_is_a_failure_not_a_state_move() {
    let mut receipt = receipt();
    receipt.expire().expect("expire");
    assert!(receipt.is_failed());
    match receipt.failure {
        Some(ReceiptFailure::Expired) => {}
        other => panic!("expected Expired, got {other:?}"),
    }
}

#[test]
fn the_ledger_keeps_one_entry_per_key_and_is_idempotent() {
    // 幂等键的意义就是"重放不会多出一条"。这里必须是幂等返回同一条，
    // 而不是报错——报错会让重试把一次正常投递变成失败。
    let mut ledger = ReceiptLedger::new();
    assert!(ledger.is_empty());
    let first = ledger.record("r1", "key-1").expect("record");
    let again = ledger.record("r1", "key-1").expect("replay");
    assert_eq!(ledger.len(), 1, "ORCH_RECEIPT_IDEMPOTENT_KEY");
    assert_eq!(first.id, again.id);
    assert_eq!(again.state, ReceiptState::Sent);
}

#[test]
fn the_ledger_advances_by_key_and_reports_missing_keys() {
    let mut ledger = ReceiptLedger::new();
    ledger.record("r1", "key-1").expect("record");
    ledger
        .advance("key-1", ReceiptState::Delivered)
        .expect("advance");
    assert_eq!(
        ledger.get("key-1").map(|r| r.state),
        Some(ReceiptState::Delivered),
        "ORCH_RECEIPT_LEDGER"
    );
    let error = ledger
        .advance("missing", ReceiptState::Delivered)
        .expect_err("missing");
    assert_eq!(error.kind(), "receipt_not_found");
}

#[test]
fn the_ledger_gets_failure_helpers_too() {
    let mut ledger = ReceiptLedger::new();
    ledger.record("r1", "key-1").expect("record");
    ledger.reject("key-1", "bad format").expect("reject");
    assert!(ledger.get("key-1").expect("present").is_failed());

    let mut other = ReceiptLedger::new();
    other.record("r2", "key-2").expect("record");
    other.expire("key-2").expect("expire");
    assert!(other.get("key-2").expect("present").is_failed());
}

fn packet(state_hash: &str, expires_at: u64) -> trit_orch::HandoffPacket {
    trit_orch::HandoffPacket {
        id: "h1".into(),
        from: "a".into(),
        to: "b".into(),
        goal: "g".into(),
        base_commit: "c0".into(),
        state_hash: state_hash.into(),
        decisions: vec![],
        open_questions: vec![],
        claims: vec![],
        evidence: vec![],
        next_action: "n".into(),
        idempotency_key: "k".into(),
        expires_at,
        signature: String::new(),
    }
}

#[test]
fn a_handoff_with_a_matching_hash_gates_true() {
    let packet = packet("sha-abc", 100);
    assert_eq!(
        handoff_gate(&packet, "sha-abc", 50),
        trit_orch::Ternary::True
    );
    assert!(
        handoff_obligation(&packet, "sha-abc", 50).is_none(),
        "通过就不欠动作"
    );
}

#[test]
fn a_hash_mismatch_is_unknown_and_owes_a_reconciliation() {
    // 收方状态和交接包声称的不一致：不能当成拒绝（对方可能只是慢了一步），
    // 必须先对账。所以是 Unknown + 一条对账义务。
    let packet = packet("sha-abc", 100);
    assert_eq!(
        handoff_gate(&packet, "sha-other", 50),
        trit_orch::Ternary::Unknown,
        "ORCH_HANDOFF_GATE"
    );
    let obligation = handoff_obligation(&packet, "sha-other", 50);
    assert!(obligation.is_some(), "ORCH_HANDOFF_OBLIGATION");
    assert_eq!(obligation.expect("obligation").kind.as_str(), "handoff");
}

#[test]
fn an_expired_packet_is_unknown_not_false() {
    // 过期可以刷新重发，属于"暂时不成立"；结构不合法才是 False。
    let packet = packet("sha-abc", 10);
    assert_eq!(
        handoff_gate(&packet, "sha-abc", 50),
        trit_orch::Ternary::Unknown,
        "ORCH_HANDOFF_EXPIRED"
    );
}

#[test]
fn a_structurally_invalid_packet_is_false() {
    let mut broken = packet("sha-abc", 100);
    broken.to = "   ".into();
    assert_eq!(
        handoff_gate(&broken, "sha-abc", 50),
        trit_orch::Ternary::False,
        "ORCH_HANDOFF_INVALID"
    );
    assert!(broken.validate(50).is_err());
}
