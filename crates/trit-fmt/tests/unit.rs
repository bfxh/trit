//! 单元级行为：空容器、单行对象、数字/字符串原文、扩展名拒绝。
//!
//! 从 `src/lib.rs` 的 `#[cfg(test)]` 移出来，给 `batch` 模块腾行数预算；
//! 这些断言只用公开 API，放在集成测试里语义不变。

use trit_fmt::{FormatError, format_json, format_path};

#[test]
fn empty_containers_have_no_padding() {
    assert_eq!(format_json("{}").unwrap(), "{}\n");
    assert_eq!(format_json("[]").unwrap(), "[]\n");
}

#[test]
fn inline_object_has_padding() {
    assert_eq!(format_json(r#"{"a":1}"#).unwrap(), "{ \"a\": 1 }\n");
}

#[test]
fn numbers_and_strings_keep_their_source_text() {
    let out = format_json(r#"{"e":1e10,"f":-0.5,"s":"a\"b\\c"}"#).unwrap();
    assert!(out.contains("1e10"));
    assert!(out.contains("-0.5"));
    assert!(out.contains(r#""a\"b\\c""#));
}

#[test]
fn rejects_non_json_syntax_by_extension() {
    assert!(matches!(
        format_path("x.js", "let a = 1;"),
        Err(FormatError::Unsupported("javascript"))
    ));
    assert!(matches!(
        format_path("x.md", "# hi"),
        Err(FormatError::Unsupported("markdown"))
    ));
}
