//! 结构化格式识别：JSON/JSONL/YAML/TOML/INI/XML/HTML/CSV/TSV/Markdown/SQL/
//! LaTeX/Dockerfile/Makefile/GraphQL/logfmt/diff/二进制。

use trit_learn::json::Json;

pub fn detect(text: &str) -> Vec<&'static str> {
    let mut out = Vec::new();
    let t = text.trim();
    if t.is_empty() {
        return out;
    }
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    let first = t.chars().next().unwrap_or(' ');
    if (first == '{' || first == '[') && Json::parse(t).is_ok() {
        out.push("json");
    }
    if lines.len() > 1 && lines.iter().all(|l| Json::parse(l).is_ok()) {
        out.push("jsonl");
    }
    if !out.contains(&"json")
        && lines.iter().any(|l| {
            let s = l.trim();
            (s.starts_with('{') || s.starts_with('[')) && Json::parse(s).is_ok()
        })
    {
        out.push("json");
    }
    let key_colon = lines
        .iter()
        .filter(|l| l.contains(':') && !l.trim_start().starts_with('#'))
        .count();
    if t.starts_with("---")
        || (key_colon as f64 / lines.len() as f64 > 0.6 && !t.starts_with('{'))
    {
        out.push("yaml");
    }
    let section = lines
        .iter()
        .any(|l| l.trim_start().starts_with('[') && l.trim_end().ends_with(']'));
    if section && lines.iter().any(|l| l.contains('=')) {
        out.push("toml");
        out.push("ini");
    }
    if t.contains("<?xml") || (t.contains('<') && t.contains("</")) {
        out.push("xml");
    }
    let lower = t.to_lowercase();
    if lower.contains("<html") || lower.contains("<!doctype") || lower.contains("<div") {
        out.push("html");
    }
    if crate::format_table::is_table(&lines, ',') {
        out.push("csv");
    }
    if crate::format_table::is_table(&lines, '\t') {
        out.push("tsv");
    }
    if lines
        .iter()
        .any(|l| l.trim_start().starts_with("# ") || l.trim_start().starts_with("- ") || l.contains("]("))
        || t.contains("```")
    {
        out.push("markdown");
    }
    let first_word = t.split_whitespace().next().unwrap_or("").to_lowercase();
    if ["select", "insert", "update", "delete", "create", "alter"].contains(&first_word.as_str()) {
        out.push("sql");
    }
    if t.contains(r"\documentclass") || t.contains(r"\begin{") || t.contains(r"\end{") {
        out.push("latex");
    }
    if lines.iter().any(|l| l.trim_start().starts_with("FROM "))
        && lines
            .iter()
            .any(|l| l.trim_start().starts_with("RUN ") || l.trim_start().starts_with("COPY "))
    {
        out.push("dockerfile");
    }
    if lines
        .iter()
        .any(|l| l.contains(':') && !l.starts_with(' ') && !l.starts_with('\t'))
        && lines.iter().any(|l| l.starts_with('\t'))
    {
        out.push("makefile");
    }
    if (lower.contains("query ") || lower.contains("mutation ") || lower.contains("type "))
        && t.contains('{')
    {
        out.push("graphql");
    }
    if lines.iter().all(|l| {
        l.split_whitespace()
            .filter(|w| w.contains('='))
            .count()
            >= 2
    }) {
        out.push("logfmt");
    }
    if t.contains("diff --git") || t.contains("@@ ") {
        out.push("diff");
    }
    let control = text
        .chars()
        .filter(|c| {
            let u = *c as u32;
            u < 0x20 && !matches!(c, '\n' | '\r' | '\t')
        })
        .count() as f64
        / text.chars().count().max(1) as f64;
    if control > 0.1 || text.contains('\0') {
        out.push("binary");
    }
    out.extend(crate::format_office::detect(t));
    out
}


#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
