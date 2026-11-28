//! 与 prettier 3.10.1 的 golden 对拍、幂等、语义保持。
//!
//! golden（`*.expected.json`）由 `npx prettier@3.10.1 --parser json <input>` 生成，
//! 生成方式与语料来源写在 `docs/specs/FRONTIER-FORMATTER-TASK.md` 的"基线"节。

use std::path::{Path, PathBuf};

use trit_fmt::{FormatError, format_json, format_path};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/json")
}

fn cases() -> Vec<(String, String, String)> {
    let dir = fixtures();
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).expect("读 fixtures").flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if !name.ends_with(".json") || name.ends_with(".expected.json") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("读输入");
        let expected_path = path.with_file_name(name.replace(".json", ".expected.json"));
        let expected = std::fs::read_to_string(&expected_path)
            .unwrap_or_else(|error| panic!("读不到 {}: {error}", expected_path.display()));
        out.push((name, source, expected));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn matches_prettier_golden() {
    let cases = cases();
    assert!(cases.len() >= 10, "语料太少：{}", cases.len());
    for (name, source, expected) in cases {
        let actual = format_json(&source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual, expected, "输出与 prettier golden 不一致：{name}");
    }
}

#[test]
fn formatting_is_idempotent() {
    for (name, source, _) in cases() {
        let once = format_json(&source).unwrap();
        let twice = format_json(&once).unwrap();
        assert_eq!(once, twice, "不幂等：{name}");
    }
}

#[test]
fn semantics_are_preserved() {
    for (name, source, _) in cases() {
        let formatted = format_json(&source).unwrap();
        let before: serde_json::Value = serde_json::from_str(&source).expect("输入合法");
        let after: serde_json::Value = serde_json::from_str(&formatted).expect("输出合法");
        assert_eq!(before, after, "语义变了：{name}");
    }
}

#[test]
fn unsupported_syntaxes_are_rejected_not_guessed() {
    for (name, source, kind) in [
        ("x.js", "let a = 1;", "javascript"),
        ("x.ts", "let a: number = 1;", "typescript"),
        ("x.txt", "hi", "unknown"),
    ] {
        match format_path(name, source) {
            Err(FormatError::Unsupported(actual)) => assert_eq!(actual, kind, "{name}"),
            other => panic!("{name} 应拒绝，却得到 {other:?}"),
        }
    }
    assert!(format_path("x.json", "{\"a\":1}").is_ok());
    assert!(format_path("x.md", "# hi").is_ok());
}

#[test]
fn syntax_errors_carry_a_position() {
    let error = format_json("{\"a\": }").expect_err("必须报错");
    match error {
        FormatError::Syntax { offset, .. } => assert!(offset > 0),
        other => panic!("应为语法错误，得到 {other:?}"),
    }
}
