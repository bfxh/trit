//! 编程语言 / 自然语言内容识别（可复算）。
//!
//! 只做**规则识别**：代码围栏、行首关键字、符号密度、常见前后端语言特征。
//! 不训练分类器；识别的语言列表用于报告，过滤只看代码占比（可配阈值）。

use crate::lang_filter::LangFilter;
use crate::script::primary_language;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Code,
    Natural,
    Mixed,
    Unknown,
}

impl ContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::Code => "code",
            ContentKind::Natural => "natural",
            ContentKind::Mixed => "mixed",
            ContentKind::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Content {
    pub code_ratio: f64,
    pub natural_ratio: f64,
    pub kind: ContentKind,
    pub code_languages: Vec<&'static str>,
}

pub fn analyze(text: &str) -> Content {
    let ratio = code_ratio(text);
    let code_languages = code_languages(text);
    let kind = if text.trim().is_empty() {
        ContentKind::Unknown
    } else if ratio >= 0.6 {
        ContentKind::Code
    } else if ratio <= 0.2 {
        ContentKind::Natural
    } else {
        ContentKind::Mixed
    };
    Content {
        code_ratio: ratio,
        natural_ratio: 1.0 - ratio,
        kind,
        code_languages,
    }
}

pub fn code_ratio(text: &str) -> f64 {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return 0.0;
    }
    let mut code_lines = 0usize;
    let mut in_fence = false;
    let mut symbols = 0usize;
    let mut chars = 0usize;
    for line in &lines {
        let t = line.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            code_lines += 1;
            continue;
        }
        if in_fence || is_code_line(t) {
            code_lines += 1;
        }
        for c in line.chars() {
            if "{}()[];=<>\\".contains(c) {
                symbols += 1;
            }
            chars += 1;
        }
    }
    let line_ratio = code_lines as f64 / lines.len() as f64;
    let symbol_ratio = symbols as f64 / chars.max(1) as f64;
    (line_ratio * 0.7 + (symbol_ratio * 2.0).min(1.0) * 0.3).min(1.0)
}

fn is_code_line(t: &str) -> bool {
    const KW: &[&str] = &[
        "fn ",
        "def ",
        "class ",
        "function ",
        "import ",
        "from ",
        "using ",
        "#include",
        "const ",
        "let ",
        "var ",
        "public ",
        "private ",
        "protected ",
        "return ",
        "if (",
        "else ",
        "for (",
        "while (",
        "switch (",
        "package ",
        "func ",
        "impl ",
        "trait ",
        "interface ",
        "namespace ",
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "CREATE ",
        "<html",
        "<div",
        "</",
        "<!DOCTYPE",
        "<?php",
        "#!/",
        "echo ",
        "export ",
        "sudo ",
        "cargo ",
        "npm ",
        "pip ",
        "docker ",
        "git ",
        "std::",
        "console.log",
        "fmt.",
        "print(",
        "printf(",
        "System.out",
        "Console.Write",
        "puts ",
        "@Override",
        "#[",
        "};",
        "});",
        "<style",
        "<script",
        "color:",
        "margin:",
        "display:",
        "@media",
        "v-if",
        "v-for",
    ];
    let starts = t.starts_with('{')
        || t.starts_with('}')
        || t.starts_with("//")
        || t.starts_with("/*")
        || (t.starts_with('#') && !t.starts_with("# "))
        || t.starts_with('$')
        || t.starts_with('>')
        || (t.ends_with(';') && t.len() > 3)
        || (t.ends_with('{') && t.len() > 3);
    starts || KW.iter().any(|k| t.starts_with(k) || t.contains(k))
}

pub fn code_languages(text: &str) -> Vec<&'static str> {
    let t = text.to_lowercase();
    let probes: &[(&str, &[&str])] = &[
        (
            "python",
            &["def ", "import ", "print(", "self.", "__init__", "elif "],
        ),
        (
            "javascript",
            &[
                "function ",
                "=>",
                "const ",
                "console.log",
                "require(",
                "document.",
            ],
        ),
        (
            "typescript",
            &["interface ", ": string", ": number", "enum ", "as const"],
        ),
        (
            "rust",
            &["fn ", "let mut", "impl ", "trait ", "cargo ", "-> "],
        ),
        (
            "java",
            &[
                "public class",
                "system.out",
                "import java",
                "@override",
                "void ",
            ],
        ),
        (
            "c",
            &["#include", "int main", "printf(", "malloc(", "stdio.h"],
        ),
        (
            "cpp",
            &["std::", "template<", "cout <<", "namespace ", "vector<"],
        ),
        (
            "csharp",
            &[
                "using system",
                "namespace ",
                "console.writeline",
                "public class",
            ],
        ),
        ("go", &["package ", "func ", "fmt.", ":="]),
        ("php", &["<?php", "echo ", "$this", "function "]),
        ("ruby", &["def ", "puts ", "require '", "end"]),
        (
            "sql",
            &["select ", "from ", "where ", "join ", "insert into"],
        ),
        ("html", &["<html", "<div", "</", "<!doctype"]),
        ("css", &["color:", "margin:", "@media", "display:"]),
        (
            "shell",
            &["#!/", "echo ", "export ", "sudo ", "apt ", "npm ", "pip "],
        ),
        ("kotlin", &["fun ", "val ", "when ("]),
        ("swift", &["func ", "guard ", "import foundation"]),
        ("scala", &["case class", "object ", "def "]),
        ("vue", &["<template>", "v-if", "v-for"]),
    ];
    probes
        .iter()
        .filter(|(_, keys)| keys.iter().any(|k| t.contains(k)))
        .map(|(lang, _)| *lang)
        .collect()
}

#[derive(Debug, Clone)]
pub struct ContentFilter {
    pub code: bool,
    pub natural: bool,
    pub threshold: f64,
    pub lang: LangFilter,
}

impl Default for ContentFilter {
    fn default() -> Self {
        Self {
            code: false,
            natural: false,
            threshold: 0.5,
            lang: LangFilter {
                enabled: false,
                ..Default::default()
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContentVerdict {
    pub content: Content,
    pub code_filtered: bool,
    pub natural_filtered: bool,
    pub lang_denied: bool,
}

impl ContentVerdict {
    pub fn filtered(&self) -> bool {
        self.code_filtered || self.natural_filtered || self.lang_denied
    }
}

impl ContentFilter {
    pub fn eval(&self, text: &str) -> ContentVerdict {
        let content = analyze(text);
        let code_filtered = self.code && content.code_ratio >= self.threshold;
        let natural_filtered = self.natural && content.natural_ratio >= self.threshold;
        let lang_denied = self.natural && self.lang.denied(primary_language(text));
        ContentVerdict {
            content,
            code_filtered,
            natural_filtered,
            lang_denied,
        }
    }
}

#[cfg(test)]
mod tests {
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
}
