//! 交接包 HMAC：签名缺失/错误/内容被改都必须判 False。

use trit_orch::{HandoffPacket, Ternary, handoff_gate, handoff_gate_with_key};

fn packet() -> HandoffPacket {
    HandoffPacket {
        id: "h1".into(),
        from: "a".into(),
        to: "b".into(),
        goal: "finish".into(),
        base_commit: "abc".into(),
        state_hash: "sha256:abc".into(),
        decisions: vec![],
        open_questions: vec![],
        claims: vec![],
        evidence: vec![],
        next_action: "run gates".into(),
        idempotency_key: "handoff:h1".into(),
        expires_at: 200,
        signature: String::new(),
    }
}

#[test]
fn handoff_signature_gate_rejects_missing_and_forged_signatures() {
    let key = b"shared-secret";
    let mut signed = packet();
    signed.sign(key);
    assert_eq!(
        handoff_gate_with_key(&signed, "sha256:abc", 100, key),
        Ternary::True
    );
    assert_eq!(
        handoff_gate_with_key(&signed, "sha256:abc", 100, b"wrong-key"),
        Ternary::False,
        "ORCH_HANDOFF_WRONG_KEY"
    );

    let unsigned = packet();
    assert_eq!(
        handoff_gate(&unsigned, "sha256:abc", 100),
        Ternary::True,
        "旧入口保持结构校验"
    );
    assert_eq!(
        handoff_gate_with_key(&unsigned, "sha256:abc", 100, key),
        Ternary::False,
        "ORCH_HANDOFF_UNSIGNED"
    );

    let mut tampered = signed.clone();
    tampered.goal = "different".into();
    assert_eq!(
        handoff_gate_with_key(&tampered, "sha256:abc", 100, key),
        Ternary::False,
        "ORCH_HANDOFF_TAMPERED"
    );
}
