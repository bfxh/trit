//! Markdown（块级子集）与 prettier 3.10.1 的 golden 对拍、幂等、明确拒绝的边界。
//!
//! golden（`*.expected.md`）由 `npx prettier@3.10.1 --parser markdown <input>` 生成，
//! 生成方式与语料来源写在 `docs/specs/FRONTIER-FORMATTER-TASK.md` 的"基线"节。

use std::path::{Path, PathBuf};

use trit_fmt::{FormatError, format_path};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/markdown")
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
        if !name.ends_with(".md") || name.ends_with(".expected.md") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("读输入");
        let expected_path = path.with_file_name(name.replace(".md", ".expected.md"));
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
    assert!(cases.len() >= 40, "语料太少：{}", cases.len());
    for (name, source, expected) in cases {
        let actual = format_path(&name, &source).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual, expected, "输出与 prettier golden 不一致：{name}");
    }
}

#[test]
fn formatting_is_idempotent() {
    for (name, source, _) in cases() {
        let once = format_path(&name, &source).unwrap();
        let twice = format_path(&name, &once).unwrap();
        assert_eq!(once, twice, "不幂等：{name}");
    }
}

#[test]
fn empty_documents_stay_empty() {
    assert_eq!(format_path("x.md", "").unwrap(), "");
    assert_eq!(format_path("x.md", "\n\n").unwrap(), "");
}

#[test]
fn explicitly_unsupported_markdown_is_rejected_not_guessed() {
    for (source, kind) in [
        ("---\ntitle: x\n---\n\n# a\n", "markdown: frontmatter"),
        ("Title\n=====\n", "markdown: setext heading"),
    ] {
        match format_path("x.md", source) {
            Err(FormatError::Unsupported(actual)) => assert_eq!(actual, kind),
            other => panic!("应拒绝，却得到 {other:?}"),
        }
    }
}
