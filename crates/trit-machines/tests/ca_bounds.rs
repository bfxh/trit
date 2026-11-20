//! 元胞自动机的宽度/步数硬顶：`vec![Ternary; width]` 与 `steps` 份历史都在分配前拒绝。
//!
//! `run` 是 pub API，`survey` 与 CLI 都走它；上限放在这里，两个入口一起收口。

use trit_machines::ca::{MAX_STEPS, MAX_WIDTH, Rule, run};

#[test]
fn at_the_caps_still_runs() {
    let rule = Rule::from_index(0).expect("规则 0");
    let out = run(&rule, MAX_WIDTH, 1);
    assert_eq!(out.cells.len(), MAX_WIDTH);
    let out = run(&rule, 8, MAX_STEPS.min(256));
    assert_eq!(out.cells.len(), 8);
}

#[test]
#[should_panic(expected = "超过上限")]
fn huge_width_panics_before_allocating() {
    let rule = Rule::from_index(0).expect("规则 0");
    let _ = run(&rule, MAX_WIDTH + 1, 1);
}

#[test]
#[should_panic(expected = "超过上限")]
fn huge_steps_panic_before_allocating() {
    let rule = Rule::from_index(0).expect("规则 0");
    let _ = run(&rule, 8, MAX_STEPS + 1);
}
