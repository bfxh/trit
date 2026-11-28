use serde_json::{Value, json};

#[allow(dead_code)]
#[path = "cli_support/mod.rs"]
mod cli_support;
use cli_support::*;

fn settled_snapshot() -> Value {
    let mut settled = snapshot(&["a"]);
    settled["observations"]["a"] = json!({
        "phase": "Settled",
        "epoch": 0,
        "authorization": "True",
        "freshness": "True",
        "evidence": {"epoch": 0, "checks": 2, "result": "Passed"},
        "usage": {"fresh": 10, "cached": 0, "output": 5}
    });
    settled
}

fn request(audit: Value) -> Value {
    json!({
        "tasks": [task("a")],
        "snapshot": settled_snapshot(),
        "rounds": [],
        "receipts": {
            "a": {
                "epoch": 0,
                "exit": {"Completed": {"code": 0}},
                "assertions_passed": 2,
                "assertions_failed": 0,
                "authorization": "True",
                "freshness": "True",
                "usage": {"fresh": 10, "cached": 0, "output": 5}
            }
        },
        "claim_audit": audit
    })
}

fn report(
    ok: bool,
    verified: u32,
    unverified: u32,
    fabricated: u32,
    claim_verdict: &str,
) -> Value {
    json!({
        "ok": ok,
        "verdict": if ok {
            "all claims verified against recorded evidence"
        } else {
            "claims did not pass the audit"
        },
        "summary": {
            "total": verified + unverified + fabricated,
            "verified": verified,
            "unverified": unverified,
            "fabricated": fabricated
        },
        "claims": [{"claim": "tests pass", "kind": "test-pass", "verdict": claim_verdict}]
    })
}

#[test]
fn passing_claim_audit_is_accepted_as_a_second_evidence_chain() {
    let (code, value) = run_with_args(
        "finish",
        &["--allow-self-reported"],
        &request(report(true, 1, 0, 0, "VERIFIED")),
    );
    assert_eq!(code, 0);
    assert_eq!(value["summary"]["confirmed"], 1);
}

#[test]
fn unverified_claim_audit_refuses_completion() {
    let (code, value) = run_with_args(
        "finish",
        &["--allow-self-reported"],
        &request(report(false, 0, 1, 0, "UNVERIFIED")),
    );
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "claim_audit_refused");
}

#[test]
fn fabricated_claim_audit_refuses_completion() {
    let (code, value) = run_with_args(
        "finish",
        &["--allow-self-reported"],
        &request(report(false, 0, 0, 1, "FABRICATED")),
    );
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "claim_audit_refused");
}

#[test]
fn self_contradictory_claim_audit_report_is_refused() {
    // ok/summary say green, but the row disagrees: the second evidence chain
    // must not trust a hand-written `ok:true`.
    let (code, value) = run_with_args(
        "finish",
        &["--allow-self-reported"],
        &request(report(true, 1, 0, 0, "UNVERIFIED")),
    );
    assert_eq!(code, 1);
    assert_eq!(value["kind"], "claim_audit_refused");

    let mut empty = report(true, 0, 0, 0, "VERIFIED");
    empty["summary"]["total"] = json!(0);
    empty["claims"] = json!([]);
    let (code, value) = run_with_args("finish", &["--allow-self-reported"], &request(empty));
    assert_eq!(code, 1);
    assert_eq!(
        value["kind"], "claim_audit_invalid",
        "an empty audit is not a second evidence chain"
    );
}
