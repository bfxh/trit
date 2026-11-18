//! `@a[…]` 的下标不做类型检查（官方语义）：`@a["x"]` 能编译，求值 undefined。

use trit_core::value::Ternary;
use trit_re::sigscan::SigScan;
use trit_re::yaraparse::{RuleOutcome, parse_rules};

#[test]
fn offset_index_does_not_type_check() {
    let src = r#"rule r { strings: $a = "x" condition: @a["x"] == 0 }"#;
    let out = parse_rules(src).expect("官方能编译（下标不查类型）");
    let RuleOutcome::Parsed(rule) = &out.rules[0] else {
        panic!("应可解析");
    };
    let report = SigScan::new(std::slice::from_ref(rule)).scan(b"x", false);
    assert_eq!(
        report.value,
        Ternary::False,
        "字符串下标 → undefined → 比较为假"
    );
}
