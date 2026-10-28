use trit_orch::{CheckReport, CheckResult, EvidenceSpec, Exit, Receipt, Ternary};

fn receipt(exit: Exit, passed: u32, failed: u32) -> Receipt {
    Receipt {
        epoch: 9,
        exit,
        assertions_passed: passed,
        assertions_failed: failed,
    }
}

fn spec() -> EvidenceSpec {
    EvidenceSpec { min_assertions: 2 }
}

fn verdict(exit: Exit, passed: u32, failed: u32) -> Ternary {
    receipt(exit, passed, failed).report(&spec()).verdict(9)
}

#[test]
fn failed_assertions_refute_regardless_of_exit_code() {
    assert_eq!(
        verdict(Exit::Completed { code: 1 }, 2, 1),
        Ternary::False,
        "ORCH_RECEIPT_NEGATIVE_FIRST"
    );
    assert_eq!(
        verdict(Exit::Completed { code: 0 }, 2, 1),
        Ternary::False,
        "ORCH_RECEIPT_NEGATIVE_FIRST"
    );
}

#[test]
fn failed_assertion_refutes_even_after_timeout() {
    assert_eq!(
        verdict(Exit::TimedOut, 2, 1),
        Ternary::False,
        "ORCH_RECEIPT_NEGATIVE_FIRST"
    );
}

#[test]
fn crashes_timeouts_and_spawn_failures_are_inconclusive_not_refuted() {
    for exit in [Exit::Signalled, Exit::TimedOut, Exit::SpawnFailed] {
        assert_eq!(
            verdict(exit, 5, 0),
            Ternary::Unknown,
            "ORCH_RECEIPT_ABNORMAL_EXIT"
        );
    }
    assert_eq!(
        verdict(Exit::Completed { code: 101 }, 5, 0),
        Ternary::Unknown,
        "ORCH_RECEIPT_NONZERO_EXIT"
    );
    assert_eq!(verdict(Exit::Completed { code: 0 }, 5, 0), Ternary::True);
}

#[test]
fn zero_assertions_never_pass() {
    assert_eq!(
        verdict(Exit::Completed { code: 0 }, 0, 0),
        Ternary::Unknown,
        "ORCH_RECEIPT_ZERO_TESTS"
    );
}

#[test]
fn insufficient_assertions_never_pass() {
    assert_eq!(
        verdict(Exit::Completed { code: 0 }, 1, 0),
        Ternary::Unknown,
        "ORCH_RECEIPT_COVERAGE"
    );
    assert_eq!(verdict(Exit::Completed { code: 0 }, 2, 0), Ternary::True);
    assert_eq!(verdict(Exit::Completed { code: 0 }, 3, 0), Ternary::True);
}

#[test]
fn stale_revision_cannot_become_support() {
    let report: CheckReport = receipt(Exit::Completed { code: 0 }, 3, 0).report(&spec());
    assert_eq!(report.epoch, 9);
    assert_eq!(report.checks, 3);
    assert_eq!(report.result, CheckResult::Passed);
    assert_eq!(
        report.verdict(8),
        Ternary::Unknown,
        "ORCH_RECEIPT_STALE_REVISION"
    );
    assert_eq!(report.verdict(9), Ternary::True);
}

#[test]
fn assertion_count_saturates_instead_of_wrapping() {
    let report = receipt(Exit::Completed { code: 0 }, u32::MAX, 1).report(&spec());
    assert_eq!(report.checks, u32::MAX);
    assert_eq!(report.result, CheckResult::Refuted);
}
