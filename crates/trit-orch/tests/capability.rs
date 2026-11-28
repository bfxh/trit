//! 动作级 capability 的端到端用例：CLI 判定 + 与 admission 五门合流。

use serde_json::json;
use trit_orch::{Ternary, admit_with_capability};

#[allow(dead_code)]
mod cli_support;
use cli_support::run;

#[test]
fn read_is_allowed_without_any_grant() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {"allow": [], "allow_untrusted_origin": false},
            "action": {"class": "read", "from_untrusted_data": true}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["ok"], true);
    assert_eq!(value["allow"], true);
    assert_eq!(value["decision"]["verdict"], "True");
    assert_eq!(value["decision"]["reason"], "read_only");
}

#[test]
fn write_without_grant_is_refused() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {"allow": ["read"]},
            "action": {"class": "write"}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["ok"], true);
    assert_eq!(value["allow"], false);
    assert_eq!(value["decision"]["verdict"], "False");
    assert_eq!(value["decision"]["reason"], "class_not_granted");
}

#[test]
fn granted_write_from_untrusted_data_needs_confirmation() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {"allow": ["read", "write"], "allow_untrusted_origin": false},
            "action": {"class": "write", "from_untrusted_data": true}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["allow"], false);
    assert_eq!(value["decision"]["verdict"], "Unknown");
    assert_eq!(
        value["decision"]["reason"],
        "untrusted_origin_requires_confirmation"
    );
}

#[test]
fn explicit_untrusted_grant_is_accepted_and_reported() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {"allow": ["exec"], "allow_untrusted_origin": true},
            "action": {"class": "exec", "from_untrusted_data": true}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["allow"], true);
    assert_eq!(value["decision"]["verdict"], "True");
    assert_eq!(value["decision"]["reason"], "granted_for_untrusted_origin");
}

#[test]
fn unknown_action_class_is_an_input_error() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {"allow": ["read"]},
            "action": {"class": "shell"}
        }),
    );
    assert_eq!(code, 1);
    assert_eq!(value["ok"], false);
    assert_eq!(value["kind"], "input_parse");
}

#[test]
fn capability_is_the_sixth_admission_gate() {
    // 五门全绿 + 动作需人确认 → 整体 Unknown，而不是被当成同意。
    assert_eq!(
        admit_with_capability(
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::Unknown,
        ),
        Ternary::Unknown
    );
    // 任一门 False 即拒绝；capability 的 False 同样一票否决。
    assert_eq!(
        admit_with_capability(
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::False,
        ),
        Ternary::False
    );
    // 已认领的 Unknown 也保持 Unknown。
    assert_eq!(
        admit_with_capability(
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::True,
            Ternary::Unknown,
            Ternary::True,
        ),
        Ternary::Unknown
    );
}
