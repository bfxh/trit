//! 编程语言 / 自然语言内容识别（可复算）。
//!
//! 只做**规则识别**：代码围栏、行首关键字、符号密度、常见前后端语言特征。
//! 不训练分类器；识别的语言列表用于报告，过滤只看代码占比（可配阈值）。

use crate::lang_filter::LangFilter;
use crate::script::primary_language;
use crate::semantics::detect as detect_semantics;
use crate::style::detect as detect_style;

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

pub use crate::code_langs::code_languages;

pub use crate::label_filter::LabelFilter;

#[derive(Debug, Clone)]
pub struct ContentFilter {
    pub code: bool,
    pub natural: bool,
    pub threshold: f64,
    pub lang: LangFilter,
    pub style: LabelFilter,
    pub semantics: LabelFilter,
    pub formats: LabelFilter,
    pub bits: LabelFilter,
    pub code_quality: bool,
    pub code_quality_limits: crate::code_quality::Limits,
    pub edu_min: f64,
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
            style: LabelFilter::default(),
            semantics: LabelFilter::default(),
            formats: LabelFilter::default(),
            bits: LabelFilter::default(),
            code_quality: false,
            code_quality_limits: crate::code_quality::Limits::default(),
            edu_min: 0.0,
        }
    }
}

pub use crate::content_verdict::ContentVerdict;

impl ContentFilter {
    pub fn eval(&self, text: &str) -> ContentVerdict {
        let content = analyze(text);
        let code_filtered = self.code && content.code_ratio >= self.threshold;
        let natural_filtered = self.natural && content.natural_ratio >= self.threshold;
        let lang_denied = self.natural && self.lang.denied(primary_language(text));
        let style = detect_style(text);
        let semantics = detect_semantics(text);
        let style_filtered = self.style.denied(style.as_str());
        let semantics_filtered = self.semantics.denied(semantics.as_str());
        let code_quality_filtered = self.code_quality
            && content.code_ratio >= self.threshold
            && !crate::code_quality::check_with(text, &self.code_quality_limits).ok;
        let edu_filtered = self.edu_min > 0.0 && crate::edu::score(text) < self.edu_min;
        let formats = crate::format::detect(text);
        let format_filtered = crate::format_filter::denied(&self.formats, &formats);
        let bits = crate::bits::labels(text);
        let bits_filtered = crate::format_filter::denied(&self.bits, &bits);
        ContentVerdict {
            content,
            style,
            semantics,
            code_filtered,
            natural_filtered,
            lang_denied,
            style_filtered,
            semantics_filtered,
            code_quality_filtered,
            edu_filtered,
            formats,
            format_filtered,
            bits,
            bits_filtered,
        }
    }
}

#[cfg(test)]
#[path = "content_tests.rs"]
mod tests;
