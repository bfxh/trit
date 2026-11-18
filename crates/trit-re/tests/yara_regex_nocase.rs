//! nocase 正则：模式与数据两侧都折叠。
//!
//! 官方 4.5.4：`$r = /HELLO/ nocase` 对 `hello` 命中（`docs/YARA-EXPR.md`
//! 第七节「既有的不一致」里的真 DIFF）。旧实现只折叠数据一侧，这里钉死。

use trit_core::value::Ternary;
use trit_re::sigscan::{Condition, Pattern, Rule, SigScan};

fn any_regex(body: &str, nocase: bool) -> Rule {
    Rule::new("r")
        .push("r", Pattern::regex(body, nocase, false, false, false))
        .condition(Condition::Any)
}

#[test]
fn nocase_regex_matches_uppercase_pattern_against_lowercase_data() {
    let rule = any_regex("HELLO", true);
    assert_eq!(
        SigScan::new(std::slice::from_ref(&rule))
            .scan(b"hello", false)
            .value,
        Ternary::True
    );
}

#[test]
fn nocase_negated_class_excludes_both_cases() {
    let rule = any_regex("[^a]+", true);
    assert_eq!(
        SigScan::new(std::slice::from_ref(&rule))
            .scan(b"aA", false)
            .value,
        Ternary::False
    );
    assert_eq!(
        SigScan::new(std::slice::from_ref(&rule))
            .scan(b"bc", false)
            .value,
        Ternary::True
    );
}
