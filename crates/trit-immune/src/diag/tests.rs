//! diag 的单元测试（从 src/diag.rs 拆出，控制单文件行数预算）。

use super::*;

const SAMPLE: &str = concat!(
    r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","code":{"code":"E0308","explanation":null},"spans":[{"file_name":"src/main.rs","line_start":3,"line_end":3,"is_primary":true}]}}"#,
    "\n",
    "\n",
    r#"{"reason":"compiler-message","message":{"level":"warning","message":"unused variable: `x`","code":null,"spans":[{"file_name":"src/lib.rs","line_start":7,"line_end":7}]}}"#,
    "\n",
    r#"{"reason":"build-finished","success":false}"#,
    "\n",
    "这不是 JSON 的进度行\n",
);

fn one(level_json: &str) -> Diagnostic {
    let line = format!(
        r#"{{"reason":"compiler-message","message":{{"level":{level_json},"message":"m","spans":[]}}}}"#
    );
    parse_rustc_stream(&line)
        .into_iter()
        .next()
        .expect("合法 compiler-message 应被解析")
}

#[test]
fn parses_rustc_stream() {
    let d = parse_rustc_stream(SAMPLE);
    assert_eq!(d.len(), 2, "build-finished 与垃圾行应被跳过");
    assert_eq!(d[0].severity, Severity::Error);
    assert_eq!(d[0].code.as_deref(), Some("E0308"));
    assert_eq!(d[0].file, "src/main.rs");
    assert_eq!(d[0].line, Some(3));
    assert_eq!(d[1].severity, Severity::Warning);
    assert_eq!(d[1].code, None);
    // 层裁决：检查类语义
    assert_eq!(layer_verdict(&d), Trit::Vulnerable);
    assert_eq!(layer_verdict(&d[1..]), Trit::Unknown);
    assert_eq!(layer_verdict(&[]), Trit::Hardened);
    assert!(has_json(SAMPLE));
    assert!(!has_json("纯文本\n没有 JSON"));
}

#[test]
fn level_variants_are_normalized_before_match() {
    // 审计 F39-11：大小写 / 首尾空白变体不得落入 Note。
    let cases = [
        (r#""ERROR""#, Severity::Error),
        (r#""Error""#, Severity::Error),
        (r#""ErRoR""#, Severity::Error),
        (r#""warning ""#, Severity::Warning),
        (r#""error\t""#, Severity::Error),
        (r#""note""#, Severity::Note),
        (r#""help""#, Severity::Note),
    ];
    for (level_json, expect) in cases {
        assert_eq!(one(level_json).severity, expect, "level={level_json}");
    }
}

#[test]
fn unrecognized_level_is_fail_closed_not_note() {
    // 未识别级别归 Unknown(0)，绝不抬成 Note-only 的 +1。
    let bad = [
        r#""fatal""#,    // 非 rustc 规范级别
        r#""""#,         // 空串
        r#""   ""#,      // 纯空白
        r#"null"#,       // 缺 level 字段
        r#"1"#,          // 级别是非字符串
        r#""致命错误""#, // 非 ASCII 乱填
    ];
    for level_json in bad {
        let d = one(level_json);
        assert_eq!(d.severity, Severity::Unknown, "level={level_json}");
        assert_eq!(
            layer_verdict(std::slice::from_ref(&d)),
            Trit::Unknown,
            "未识别级别不得判 +1：level={level_json}"
        );
    }
}

#[test]
fn unknown_severity_does_not_elevate_note_only_verdict() {
    let note = one(r#""note""#);
    let unknown = one(r#""fatal""#);
    // Note-only 仍是 +1；混入一条未识别级别即降到 0，不放行。
    assert_eq!(layer_verdict(std::slice::from_ref(&note)), Trit::Hardened);
    assert_eq!(layer_verdict(&[note, unknown.clone()]), Trit::Unknown);
    // 任一变体 error 仍压成 -1。
    let err = one(r#""ERROR""#);
    assert_eq!(layer_verdict(&[err, unknown]), Trit::Vulnerable);
}
