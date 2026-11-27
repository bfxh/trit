//! F5 回归：空栈 `HALT` 与 `DUP` / `JZ` 空栈口径一致——
//! `truncated=true, halted=false, verdict=null, outputs 为空`，不得伪造 `Unknown` 输出。

use trit_core::value::Ternary;
use trit_machines::vm::{Limits, Op, RunReport, run};

fn assert_empty_stack_consistent(report: &RunReport, op: &str) {
    assert!(report.truncated, "{op}: 应判 truncated");
    assert!(!report.halted, "{op}: halted 必须为 false");
    assert!(report.verdict().is_none(), "{op}: verdict 必须为 null");
    assert!(report.outputs.is_empty(), "{op}: 不得伪造输出");
}

#[test]
fn empty_stack_halt_is_truncated_not_unknown() {
    let report = run(&[Op::Halt], Limits::default());
    assert_empty_stack_consistent(&report, "HALT");
}

#[test]
fn empty_stack_dup_is_truncated() {
    let report = run(&[Op::Dup], Limits::default());
    assert_empty_stack_consistent(&report, "DUP");
}

#[test]
fn empty_stack_jz_is_truncated() {
    let report = run(&[Op::Jz(0)], Limits::default());
    assert_empty_stack_consistent(&report, "JZ");
}

#[test]
fn halt_with_value_is_still_definite() {
    let report = run(&[Op::Push(Ternary::True), Op::Halt], Limits::default());
    assert!(!report.truncated);
    assert!(report.halted);
    assert_eq!(report.verdict(), Some(Ternary::True));
    assert_eq!(report.outputs[&Ternary::True], 1);
}
