//! JSONL 台账。
//!
//! 三类行：`file`（样本头）、`fact`（一条三值观测）、`judgment`（一条判定）。
//! 每行都带 `sha256`，所以同一文件的多行可以独立取样、独立消费；
//! `schema` 只出现在 file 行，改结构就改它。

use trit_belnap::B4;
use trit_core::value::Ternary;

use crate::json;
use crate::triage::Triage;

/// 台账结构版本。
pub const SCHEMA: u32 = 1;

/// 三值的稳定字符串（进台账，不要改）。
pub fn ternary_label(value: Ternary) -> &'static str {
    match value {
        Ternary::True => "true",
        Ternary::False => "false",
        Ternary::Unknown => "unknown",
    }
}

/// 四值的稳定字符。
pub fn four_label(value: B4) -> String {
    value.as_char().to_string()
}

/// 渲染成若干 JSONL 行。
pub fn jsonl(triage: &Triage) -> Vec<String> {
    let mut lines = Vec::with_capacity(2 + triage.facts.len() + triage.judgments.len());
    lines.push(json::object(&[
        ("kind", json::string("file")),
        ("schema", json::number(SCHEMA)),
        ("sha256", json::string(&triage.sha256)),
        ("size", json::number(triage.size)),
        ("format", json::string(&triage.format)),
        ("arch", json::string(&triage.arch)),
        ("sections", json::number(triage.sections)),
        (
            "imported_functions",
            match triage.imported_functions {
                Some(count) => json::number(count),
                None => "null".to_string(),
            },
        ),
    ]));
    for witness in triage.facts.iter() {
        lines.push(json::object(&[
            ("kind", json::string("fact")),
            ("sha256", json::string(&triage.sha256)),
            ("id", json::string(witness.id)),
            ("value", json::string(ternary_label(witness.value))),
            ("detail", json::string(&witness.detail)),
        ]));
    }
    for judgment in &triage.judgments {
        lines.push(json::object(&[
            ("kind", json::string("judgment")),
            ("sha256", json::string(&triage.sha256)),
            ("id", json::string(judgment.id)),
            ("title", json::string(judgment.title)),
            ("value", json::string(ternary_label(judgment.value))),
            ("four", json::string(&four_label(judgment.four))),
            (
                "support",
                json::array(judgment.supporting.iter().map(String::as_str)),
            ),
            (
                "refute",
                json::array(judgment.refuting.iter().map(String::as_str)),
            ),
            (
                "unknown",
                json::array(judgment.undecided.iter().map(String::as_str)),
            ),
        ]));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{SCHEMA, jsonl, ternary_label};
    use crate::triage::analyze;
    use trit_core::value::Ternary;

    #[test]
    fn labels_are_stable() {
        assert_eq!(ternary_label(Ternary::True), "true");
        assert_eq!(ternary_label(Ternary::False), "false");
        assert_eq!(ternary_label(Ternary::Unknown), "unknown");
    }

    #[test]
    fn every_line_is_a_self_contained_object() {
        let triage = analyze(b"format unknown");
        let lines = jsonl(&triage);
        assert_eq!(lines.len(), 1 + triage.facts.len() + triage.judgments.len());
        for line in &lines {
            assert!(line.starts_with('{') && line.ends_with('}'), "{line}");
            assert!(line.contains("\"sha256\":\""), "{line}");
        }
        assert!(lines[0].contains(&format!("\"schema\":{SCHEMA}")));
        assert!(lines[0].contains("\"kind\":\"file\""));
    }
}
