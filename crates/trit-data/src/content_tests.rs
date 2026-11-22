//! content 单测。

use super::*;

const PY: &str = "def solve(x):\n    return x + 1\n\nprint(solve(2))";
const WEB: &str = "<html>\n<div class=\"app\"></div>\n<script>\nconst el = document.querySelector('.app');\n</script>\n<style>\n.app { color: red; }\n</style>";
const SQL: &str = "SELECT id, name FROM users WHERE age > 18;";
const NAT: &str = "这是一段普通的自然语言说明，描述今天下午的天气和窗外的树影。";

#[test]
fn detects_python_and_kind() {
    let c = analyze(PY);
    assert_eq!(c.kind, ContentKind::Code);
    assert!(
        c.code_languages.contains(&"python"),
        "{:?}",
        c.code_languages
    );
    assert!(c.code_ratio > 0.5);
}

#[test]
fn detects_frontend_and_sql() {
    let w = analyze(WEB);
    assert_eq!(w.kind, ContentKind::Code);
    assert!(w.code_languages.contains(&"html"), "{:?}", w.code_languages);
    assert!(w.code_languages.contains(&"javascript"));
    assert!(w.code_languages.contains(&"css"));
    let s = analyze(SQL);
    assert!(s.code_languages.contains(&"sql"));
}

#[test]
fn natural_text_is_natural() {
    let c = analyze(NAT);
    assert_eq!(c.kind, ContentKind::Natural);
    assert!(c.code_ratio < 0.2);
}

#[test]
fn filters_default_off_and_switches_are_independent() {
    let f = ContentFilter::default();
    assert!(!f.eval(PY).filtered());
    assert!(!f.eval(NAT).filtered());

    let code = ContentFilter {
        code: true,
        ..ContentFilter::default()
    };
    assert!(code.eval(PY).code_filtered);
    assert!(!code.eval(NAT).code_filtered);

    let nat = ContentFilter {
        natural: true,
        ..ContentFilter::default()
    };
    assert!(nat.eval(NAT).natural_filtered);
    assert!(!nat.eval(PY).natural_filtered);

    let both = ContentFilter {
        code: true,
        natural: true,
        ..ContentFilter::default()
    };
    assert!(both.eval(PY).code_filtered);
    assert!(both.eval(NAT).natural_filtered);
}

#[test]
fn language_deny_only_applies_when_natural_filter_on() {
    let f = ContentFilter {
        natural: true,
        lang: LangFilter {
            enabled: true,
            deny: vec!["en".into()],
            ..Default::default()
        },
        ..ContentFilter::default()
    };
    assert!(
        f.eval("This is an English sentence about the weather.")
            .lang_denied
    );
    let no_nat = ContentFilter {
        natural: false,
        ..f.clone()
    };
    assert!(
        !no_nat
            .eval("This is an English sentence about the weather.")
            .lang_denied
    );
}
