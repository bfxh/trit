use trit_orch::{
    Claim, ClaimOutcome, ClaimState, ClaimStore, DeliveryReceipt, HandoffPacket, MAX_CLAIM_PATHS,
    ReceiptFailure, ReceiptLedger, ReceiptState, Ternary, admit_with_claim, handoff_gate,
    paths_overlap,
};

fn paths(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn acquired(outcome: ClaimOutcome) -> Claim {
    match outcome {
        ClaimOutcome::Acquired(claim) => claim,
        other => panic!("expected acquired claim, got {other:?}"),
    }
}

#[test]
fn claim_acquire_renew_release_round_trip() {
    let mut store = ClaimStore::new();
    let claim = acquired(
        store
            .claim("alice", &paths(&["src/a.rs"]), "edit", 100, 10)
            .unwrap(),
    );
    assert_eq!(claim.fencing, 1);
    assert_eq!(store.state_of(&claim.id, 105), ClaimState::Held);
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "alice", 105).unwrap(),
        Ternary::True
    );
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "bob", 105).unwrap(),
        Ternary::False
    );

    let renewed = store
        .renew(&claim.id, "alice", claim.fencing, 105, 20)
        .unwrap();
    assert!(renewed.fencing > claim.fencing);
    assert_eq!(renewed.expires_at, 125);
    assert!(
        store.release(&claim.id, "alice", claim.fencing).is_err(),
        "ORCH_CLAIM_FENCE: old fence must not release"
    );
    store.release(&claim.id, "alice", renewed.fencing).unwrap();
    assert_eq!(store.state_of(&claim.id, 105), ClaimState::Free);
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "alice", 105).unwrap(),
        Ternary::Unknown,
        "ORCH_CLAIM_COVERAGE: released paths are not covered"
    );
}

#[test]
fn claim_conflict_is_contested_and_gate_blocks() {
    let mut store = ClaimStore::new();
    let first = acquired(
        store
            .claim("alice", &paths(&["src/a.rs"]), "edit", 100, 10)
            .unwrap(),
    );
    match store
        .claim("bob", &paths(&["src/a.rs"]), "edit", 101, 10)
        .unwrap()
    {
        ClaimOutcome::Contested { contenders, .. } => {
            assert_eq!(contenders, vec![first.id.clone()], "ORCH_CLAIM_CONTESTED");
        }
        other => panic!("expected contested, got {other:?}"),
    }
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "bob", 101).unwrap(),
        Ternary::False
    );
    assert_eq!(
        store.state_of_paths(&paths(&["src/a.rs"]), 101).unwrap(),
        ClaimState::Held
    );
}

#[test]
fn expired_claim_requires_reclaim_and_blocks_gate() {
    let mut store = ClaimStore::new();
    let old = acquired(
        store
            .claim("alice", &paths(&["src/a.rs"]), "edit", 100, 10)
            .unwrap(),
    );
    assert_eq!(
        store.state_of(&old.id, 111),
        ClaimState::Expired,
        "ORCH_CLAIM_EXPIRED"
    );
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "alice", 111).unwrap(),
        Ternary::Unknown,
        "ORCH_CLAIM_EXPIRED: expired is not free"
    );
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "bob", 111).unwrap(),
        Ternary::Unknown,
        "ORCH_CLAIM_EXPIRED: expired still needs reclaim"
    );
    assert_eq!(
        store
            .renew(&old.id, "alice", old.fencing, 111, 10)
            .expect_err("ORCH_CLAIM_EXPIRED: renew must not resurrect")
            .kind(),
        "claim_expired",
        "ORCH_CLAIM_EXPIRED: renew must not resurrect"
    );
    let fresh = acquired(
        store
            .claim("bob", &paths(&["src/a.rs"]), "takeover", 111, 10)
            .unwrap(),
    );
    assert!(fresh.fencing > old.fencing);
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "bob", 111).unwrap(),
        Ternary::True
    );
}

#[test]
fn stale_fence_cannot_renew_or_release() {
    let mut store = ClaimStore::new();
    let claim = acquired(
        store
            .claim("alice", &paths(&["src/a.rs"]), "edit", 100, 10)
            .unwrap(),
    );
    assert_eq!(
        store
            .renew(&claim.id, "alice", 999, 101, 10)
            .expect_err("ORCH_CLAIM_FENCE: stale renew must fail")
            .kind(),
        "stale_fence",
        "ORCH_CLAIM_FENCE"
    );
    assert_eq!(
        store
            .release(&claim.id, "alice", 999)
            .expect_err("ORCH_CLAIM_FENCE: stale release must fail")
            .kind(),
        "stale_fence",
        "ORCH_CLAIM_FENCE"
    );
}

#[test]
fn claim_path_prefix_conflict_is_detected() {
    assert!(paths_overlap("src", "src/a.rs"));
    assert!(paths_overlap("src/a.rs", "src"));
    assert!(!paths_overlap("src/a.rs", "src/b.rs"));
    assert!(paths_overlap(
        "crates/trit-orch",
        "crates/trit-orch/src/lib.rs"
    ));

    let mut store = ClaimStore::new();
    let first = acquired(
        store
            .claim("alice", &paths(&["src/a.rs"]), "edit", 100, 10)
            .unwrap(),
    );
    match store
        .claim("bob", &paths(&["src"]), "edit", 101, 10)
        .unwrap()
    {
        ClaimOutcome::Contested { contenders, .. } => {
            assert_eq!(contenders, vec![first.id.clone()], "ORCH_CLAIM_PREFIX");
        }
        other => panic!("ORCH_CLAIM_PREFIX: expected prefix conflict, got {other:?}"),
    }
    assert!(
        matches!(
            store
                .claim("carol", &paths(&["src/b.rs"]), "edit", 101, 10)
                .unwrap(),
            ClaimOutcome::Acquired(_)
        ),
        "ORCH_CLAIM_PREFIX: sibling path must stay free"
    );

    // gate 侧：别人持有宽认领 `src` 时，窄请求 `src/a.rs` 必须被挡。
    let mut broad = ClaimStore::new();
    let _ = acquired(
        broad
            .claim("alice", &paths(&["src"]), "edit", 100, 10)
            .unwrap(),
    );
    assert_eq!(
        broad.gate(&paths(&["src/a.rs"]), "bob", 101).unwrap(),
        Ternary::False,
        "ORCH_CLAIM_PREFIX: broad claim must block a narrow request"
    );
}

#[test]
fn claim_gate_requires_full_coverage() {
    let mut store = ClaimStore::new();
    let _ = acquired(
        store
            .claim("alice", &paths(&["src/a.rs"]), "edit", 100, 10)
            .unwrap(),
    );
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "alice", 101).unwrap(),
        Ternary::True
    );
    assert_eq!(
        store.gate(&paths(&["src"]), "alice", 101).unwrap(),
        Ternary::Unknown,
        "ORCH_CLAIM_COVERAGE: narrow claim cannot cover parent"
    );
    assert_eq!(
        store.gate(&paths(&["src/b.rs"]), "alice", 101).unwrap(),
        Ternary::Unknown,
        "ORCH_CLAIM_COVERAGE: unrelated path is not covered"
    );
}

#[test]
fn claim_input_validation_fails_closed() {
    let mut store = ClaimStore::new();
    assert_eq!(
        store
            .claim("", &paths(&["a"]), "x", 0, 1)
            .unwrap_err()
            .kind(),
        "claim_blank_owner"
    );
    assert_eq!(
        store.claim("a", &[], "x", 0, 1).unwrap_err().kind(),
        "claim_blank_path"
    );
    assert_eq!(
        store
            .claim("a", &paths(&["/abs"]), "x", 0, 1)
            .unwrap_err()
            .kind(),
        "claim_path_escape"
    );
    assert_eq!(
        store
            .claim("a", &paths(&["a/../b"]), "x", 0, 1)
            .unwrap_err()
            .kind(),
        "claim_path_escape"
    );
    assert_eq!(
        store
            .claim("a", &paths(&["*"]), "x", 0, 1)
            .unwrap_err()
            .kind(),
        "claim_path_escape"
    );
    assert_eq!(
        store
            .claim("a", &paths(&["a"]), "x", 0, 0)
            .unwrap_err()
            .kind(),
        "claim_zero_ttl"
    );
    assert_eq!(
        store
            .claim("a", &paths(&["a"]), "", 0, 1)
            .unwrap_err()
            .kind(),
        "claim_intent_blank"
    );
    let many: Vec<String> = (0..=MAX_CLAIM_PATHS).map(|i| format!("p/{i}")).collect();
    assert_eq!(
        store.claim("a", &many, "x", 0, 1).unwrap_err().kind(),
        "claim_too_many_paths"
    );
}

#[test]
fn claim_from_claims_reports_contested() {
    let alice = Claim {
        id: "a".into(),
        owner: "alice".into(),
        paths: paths(&["src"]),
        intent: "edit".into(),
        fencing: 1,
        acquired_at: 0,
        expires_at: 100,
    };
    let bob = Claim {
        id: "b".into(),
        owner: "bob".into(),
        paths: paths(&["src/a.rs"]),
        intent: "edit".into(),
        fencing: 2,
        acquired_at: 0,
        expires_at: 100,
    };
    let store = ClaimStore::from_claims(vec![alice, bob]).unwrap();
    assert_eq!(
        store.state_of_paths(&paths(&["src/a.rs"]), 50).unwrap(),
        ClaimState::Contested,
        "ORCH_CLAIM_CONTESTED: recovered overlap must be visible"
    );
    assert_eq!(
        store.gate(&paths(&["src/a.rs"]), "alice", 50).unwrap(),
        Ternary::False
    );
}

#[test]
fn receipt_four_states_are_strict_and_idempotent() {
    let mut receipt = DeliveryReceipt::sent("r1", "k1").unwrap();
    assert_eq!(receipt.state, ReceiptState::Sent);
    assert_eq!(
        receipt
            .advance(ReceiptState::Read)
            .expect_err("ORCH_RECEIPT_SKIP: Sent cannot jump to Read")
            .kind(),
        "receipt_skip",
        "ORCH_RECEIPT_SKIP"
    );
    receipt.advance(ReceiptState::Delivered).unwrap();
    receipt.advance(ReceiptState::Delivered).unwrap();
    receipt.advance(ReceiptState::Read).unwrap();
    receipt.advance(ReceiptState::Acted).unwrap();
    assert!(receipt.is_acted());
    receipt.advance(ReceiptState::Acted).unwrap();
    assert_eq!(
        receipt
            .reject("no")
            .expect_err("ORCH_RECEIPT_TERMINAL: acted receipt cannot reject")
            .kind(),
        "receipt_already_acted",
        "ORCH_RECEIPT_TERMINAL"
    );
    assert_eq!(
        receipt
            .expire()
            .expect_err("ORCH_RECEIPT_TERMINAL: acted receipt cannot expire")
            .kind(),
        "receipt_already_acted",
        "ORCH_RECEIPT_TERMINAL"
    );
}

#[test]
fn receipt_failure_is_terminal() {
    let mut rejected = DeliveryReceipt::sent("r1", "k1").unwrap();
    rejected.reject("bad payload").unwrap();
    assert_eq!(rejected.failure, Some(ReceiptFailure::Rejected));
    rejected.reject("again").unwrap();
    assert_eq!(
        rejected
            .advance(ReceiptState::Delivered)
            .unwrap_err()
            .kind(),
        "receipt_failed",
        "ORCH_RECEIPT_TERMINAL"
    );
    assert_eq!(
        rejected.expire().unwrap_err().kind(),
        "receipt_failed",
        "ORCH_RECEIPT_TERMINAL"
    );

    let mut expired = DeliveryReceipt::sent("r2", "k2").unwrap();
    expired.expire().unwrap();
    assert_eq!(expired.failure, Some(ReceiptFailure::Expired));
    assert_eq!(
        expired.advance(ReceiptState::Delivered).unwrap_err().kind(),
        "receipt_failed"
    );
}

#[test]
fn receipt_ledger_dedups_by_idempotency_key() {
    let mut ledger = ReceiptLedger::new();
    let first = ledger.record("r1", "k1").unwrap();
    assert_eq!(first.state, ReceiptState::Sent);
    let duplicate = ledger.record("r2", "k1").unwrap();
    assert_eq!(duplicate.id, "r1");
    ledger.advance("k1", ReceiptState::Delivered).unwrap();
    ledger.advance("k1", ReceiptState::Delivered).unwrap();
    assert_eq!(ledger.len(), 1);
    assert_eq!(ledger.get("k1").unwrap().state, ReceiptState::Delivered);
    assert_eq!(
        ledger
            .advance("missing", ReceiptState::Read)
            .unwrap_err()
            .kind(),
        "receipt_not_found"
    );
}

fn valid_packet() -> HandoffPacket {
    HandoffPacket {
        id: "h1".into(),
        from: "alice".into(),
        to: "bob".into(),
        goal: "finish trit-re split".into(),
        base_commit: "0725d92".into(),
        state_hash: "sha256:abc".into(),
        decisions: vec!["keep 31 mutants".into()],
        open_questions: vec!["who owns trit-re.json".into()],
        claims: vec!["claim-7".into()],
        evidence: vec!["commit:0725d92".into()],
        next_action: "run gates".into(),
        idempotency_key: "handoff:h1".into(),
        expires_at: 200,
    }
}

#[test]
fn handoff_gate_uses_state_hash_and_expiry() {
    assert_eq!(
        handoff_gate(&valid_packet(), "sha256:abc", 100),
        Ternary::True
    );
    assert_eq!(
        handoff_gate(&valid_packet(), "sha256:other", 100),
        Ternary::Unknown,
        "ORCH_HANDOFF_STATE"
    );
    let mut expired = valid_packet();
    expired.expires_at = 50;
    assert_eq!(
        handoff_gate(&expired, "sha256:abc", 100),
        Ternary::Unknown,
        "ORCH_HANDOFF_EXPIRED"
    );
    let mut invalid = valid_packet();
    invalid.goal.clear();
    assert_eq!(
        handoff_gate(&invalid, "sha256:abc", 100),
        Ternary::False,
        "ORCH_HANDOFF_BLANK"
    );
}

#[test]
fn handoff_validation_rejects_blank_fields_and_entries() {
    let mut packet = valid_packet();
    packet.claims.push("  ".into());
    assert_eq!(
        packet.validate(100).unwrap_err().kind(),
        "handoff_field_blank"
    );
    let mut packet = valid_packet();
    packet.idempotency_key.clear();
    assert_eq!(
        packet.validate(100).unwrap_err().kind(),
        "handoff_field_blank"
    );
    let mut packet = valid_packet();
    packet.expires_at = 100;
    assert_eq!(packet.validate(100).unwrap_err().kind(), "handoff_expired");
}

#[test]
fn admit_with_claim_matches_k3_and_over_all_243_inputs() {
    use Ternary::{False as F, True as T, Unknown as U};
    for dependencies in [F, U, T] {
        for authorization in [F, U, T] {
            for resources in [F, U, T] {
                for freshness in [F, U, T] {
                    for claim in [F, U, T] {
                        let inputs = [dependencies, authorization, resources, freshness, claim];
                        let expected = if inputs.contains(&F) {
                            F
                        } else if inputs.contains(&U) {
                            U
                        } else {
                            T
                        };
                        assert_eq!(
                            admit_with_claim(
                                dependencies,
                                authorization,
                                resources,
                                freshness,
                                claim
                            ),
                            expected,
                            "ORCH_ADMISSION_CLAIM_GATE: {inputs:?}"
                        );
                    }
                }
            }
        }
    }
}
