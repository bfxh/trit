//! P2 双轨代数的验收：冲突必须单独存在，且不能被多数票或后到的写入消掉。

use crate::*;

fn report(result: CheckResult) -> CheckReport {
    CheckReport {
        epoch: 0,
        checks: 1,
        result,
    }
}

#[test]
fn single_rail_decides_and_empty_stays_unknown() {
    let mut rails = Rails::empty();
    assert_eq!(rails.state(), EvidenceVerdict::Neither);
    assert_eq!(rails.judge(), Ternary::Unknown);

    rails.observe(EvidenceVerdict::Proved);
    assert_eq!(rails.state(), EvidenceVerdict::Proved);
    assert_eq!(rails.judge(), Ternary::True);

    let mut rails = Rails::empty();
    rails.observe(EvidenceVerdict::Refuted);
    assert_eq!(rails.state(), EvidenceVerdict::Refuted);
    assert_eq!(rails.judge(), Ternary::False);
}

#[test]
fn both_rails_are_conflict_not_absence() {
    let mut rails = Rails::empty();
    rails.observe(EvidenceVerdict::Proved);
    rails.observe(EvidenceVerdict::Refuted);

    // 三值口径都是 Unknown，但四值口径必须能分开：这是"B 单独存在"。
    assert_eq!(rails.judge(), Ternary::Unknown);
    assert_eq!(rails.state(), EvidenceVerdict::Conflicted);
    assert!(rails.is_conflicted());
    assert_ne!(rails.state(), EvidenceVerdict::Neither);
}

#[test]
fn majority_never_wins() {
    let mut rails = Rails::empty();
    for _ in 0..3 {
        rails.observe(EvidenceVerdict::Proved);
    }
    rails.observe(EvidenceVerdict::Refuted);

    assert_eq!(rails.support, 3);
    assert_eq!(rails.oppose, 1);
    assert_eq!(
        rails.state(),
        EvidenceVerdict::Conflicted,
        "3:1 仍然是冲突，不是通过"
    );
    assert_eq!(rails.judge(), Ternary::Unknown);
}

#[test]
fn last_write_does_not_win() {
    // 后到的反对票不会覆盖支持轨，也不会把结论翻成 False。
    let mut rails = Rails::empty();
    rails.observe(EvidenceVerdict::Proved);
    rails.observe(EvidenceVerdict::Refuted);

    assert_eq!(rails.support, 1, "先前那条支持证据必须留着");
    assert_eq!(rails.state(), EvidenceVerdict::Conflicted);
}

#[test]
fn self_declared_conflict_marks_both_rails() {
    let mut rails = Rails::empty();
    rails.observe_report(&report(CheckResult::Conflicted), 0);

    assert!(rails.support > 0 && rails.oppose > 0);
    assert_eq!(rails.state(), EvidenceVerdict::Conflicted);
}

#[test]
fn stale_or_empty_reports_do_not_vote() {
    let mut rails = Rails::empty();
    rails.observe(EvidenceVerdict::Proved);

    // epoch 不符：旧证据作废，不许投反对票。
    rails.observe_report(&report(CheckResult::Refuted), 9);
    // checks == 0：没跑过断言，等于没证据。
    rails.observe_report(
        &CheckReport {
            epoch: 0,
            checks: 0,
            result: CheckResult::Refuted,
        },
        0,
    );

    assert_eq!(
        rails.state(),
        EvidenceVerdict::Proved,
        "过期证据不得污染当前轨道"
    );
}

#[test]
fn merge_is_commutative_and_preserves_conflict() {
    let mut left = Rails::empty();
    left.observe(EvidenceVerdict::Proved);
    left.observe(EvidenceVerdict::Refuted);

    let mut right = Rails::empty();
    for _ in 0..5 {
        right.observe(EvidenceVerdict::Proved);
    }

    let merged = left.merge(right);
    let flipped = right.merge(left);
    assert_eq!(merged, flipped, "合并必须可交换");
    assert_eq!(
        merged.state(),
        EvidenceVerdict::Conflicted,
        "再补 5 份支持也不能消掉冲突"
    );
    assert_eq!(merged.support, 6);
    assert_eq!(merged.oppose, 1);
}

#[test]
fn ledger_accumulates_per_subject_and_lists_conflicts() {
    let mut ledger = RailsLedger::new();
    ledger.observe("a", &report(CheckResult::Passed), 0);
    ledger.observe("a", &report(CheckResult::Refuted), 0);
    ledger.observe("b", &report(CheckResult::Passed), 0);
    ledger.observe("c", &report(CheckResult::Inconclusive), 0);

    assert_eq!(ledger.len(), 3);
    assert_eq!(ledger.conflicted(), vec!["a"]);
    assert_eq!(ledger.judge("a"), Ternary::Unknown);
    assert_eq!(ledger.judge("b"), Ternary::True);
    assert_eq!(ledger.judge("c"), Ternary::Unknown);
    assert_eq!(
        ledger.unevidenced(),
        vec!["c"],
        "Inconclusive 是没证据，不是反对"
    );
}

#[test]
fn rebase_is_the_only_way_to_clear_a_conflict() {
    let mut ledger = RailsLedger::new();
    ledger.observe("a", &report(CheckResult::Passed), 0);
    ledger.observe("a", &report(CheckResult::Refuted), 0);
    assert!(ledger.is_conflicted("a"));

    ledger.rebase("a");
    assert_eq!(ledger.judgment("a"), EvidenceVerdict::Neither);
    assert!(!ledger.is_conflicted("a"));
}

#[test]
fn merging_ledgers_never_loses_a_rail() {
    let mut host_a = RailsLedger::new();
    host_a.observe("x", &report(CheckResult::Passed), 0);
    let mut host_b = RailsLedger::new();
    host_b.observe("x", &report(CheckResult::Refuted), 0);

    host_a.merge(host_b);
    assert_eq!(host_a.judge("x"), Ternary::Unknown);
    assert_eq!(host_a.judgment("x"), EvidenceVerdict::Conflicted);
    assert_eq!(host_a.rails("x"), Rails::new(1, 1));
}
