//! raw.rs 的测试（拆出守模块行数）。

use crate::lexer::scrub;

fn joined(source: &str) -> String {
    scrub(source).join("\n")
}

#[test]
fn strips_line_and_block_comments() {
    let text = joined("let a = 1; // unsafe { x }\n/* unsafe { y } */ let b = 2;\n");
    assert!(!text.contains("unsafe"), "{text:?}");
    assert!(text.contains("let a = 1;"));
    assert!(text.contains("let b = 2;"));
}

#[test]
fn strips_strings_and_raw_strings() {
    let text = joined("let s = \"unsafe { a }\"; let r = r#\"unsafe { b }\"#; let q = 1;\n");
    assert!(!text.contains("unsafe"), "{text:?}");
    assert!(text.contains("let q = 1;"), "{text:?}");
}

#[test]
fn keeps_line_count_and_real_code() {
    let lines = scrub("fn a() {}\nunsafe { go(); }\n");
    assert_eq!(lines.len(), 3, "末尾换行会多一行空行");
    assert!(lines[1].contains("unsafe"));
}

#[test]
fn nested_block_comments_do_not_leak() {
    let text = joined("/* a /* b */ unsafe { c } */ let d = 1;\n");
    assert!(!text.contains("unsafe"), "{text:?}");
    assert!(text.contains("let d = 1;"));
}

#[test]
fn char_literal_quote_does_not_hide_unsafe() {
    // 第 2 轮 U1：`'"'` 曾被当成字符串起点，把同一行的 unsafe 整段抹成空格
    let lines = scrub(
        "fn f(p: *const u8) -> u8 {\n    let q = '\"'; let v = unsafe { std::ptr::read(p) }; let _ = q; v\n}\n",
    );
    assert!(
        lines[1].contains("unsafe"),
        "字符字面量不应抹掉同行的 unsafe：{:?}",
        lines[1]
    );
}
