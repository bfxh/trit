//! 目标白名单的 CLI 用例：精确匹配、越界拒绝、缺目标拒绝。

use serde_json::json;

#[allow(dead_code)]
mod cli_support;
use cli_support::run;

#[test]
fn target_whitelist_allows_exact_match() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {
                "allow": ["write"],
                "allow_targets": {"write": ["docs/out.md"]}
            },
            "action": {"class": "write", "target": "docs/out.md"}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["allow"], true);
    assert_eq!(value["decision"]["verdict"], "True");
    assert_eq!(value["decision"]["target"], "docs/out.md");
}

#[test]
fn target_outside_whitelist_is_refused() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {
                "allow": ["write"],
                "allow_targets": {"write": ["docs/out.md"]}
            },
            "action": {"class": "write", "target": "..\\etc\\passwd"}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["allow"], false);
    assert_eq!(value["decision"]["verdict"], "False");
    assert_eq!(value["decision"]["reason"], "target_not_allowed");
}

#[test]
fn target_whitelist_requires_a_target() {
    let (code, value) = run(
        "capability",
        &json!({
            "policy": {
                "allow": ["write"],
                "allow_targets": {"write": ["docs/out.md"]}
            },
            "action": {"class": "write"}
        }),
    );
    assert_eq!(code, 0);
    assert_eq!(value["allow"], false);
    assert_eq!(value["decision"]["reason"], "target_required");
}
