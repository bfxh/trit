//! JSONL output.
//!
//! The schema is a contract: [`SCHEMA_VERSION`] changes whenever a field is renamed
//! or removed, never when one is added. Two deliberate choices:
//!
//! * Qualifiers are single characters (`T`/`F`/`U`/`B`), not words, so a consumer
//!   cannot accidentally parse them as booleans — `false` and `"unknown"` are easy to
//!   confuse at a glance, `F` and `U` are not.
//! * Numbers are separated from qualifiers. A measurement and a decision are
//!   different kinds of object and mixing them into one field invites treating the
//!   number as the answer.

use crate::scan::Record;

/// Version of the emitted schema.
pub const SCHEMA_VERSION: u32 = 1;

/// Quotes and escapes a string for JSON.
#[must_use]
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Serialises one record.
#[must_use]
pub fn record(rec: &Record) -> String {
    let mut out = String::with_capacity(1024);
    out.push_str("{\"schema\":");
    out.push_str(&SCHEMA_VERSION.to_string());
    out.push_str(",\"path\":");
    out.push_str(&escape(&rec.path));
    out.push_str(",\"size\":");
    out.push_str(&rec.size.to_string());
    out.push_str(",\"kind\":");
    out.push_str(&escape(rec.kind));
    out.push_str(",\"elapsed_us\":");
    out.push_str(&rec.elapsed_us.to_string());

    match &rec.error {
        Some(err) => {
            out.push_str(",\"error\":");
            out.push_str(&escape(err));
        }
        None => out.push_str(",\"error\":null"),
    }

    out.push_str(",\"verdict\":{\"lo\":");
    out.push_str(&escape(rec.verdict.lo.name()));
    out.push_str(",\"hi\":");
    out.push_str(&escape(rec.verdict.hi.name()));
    out.push_str(",\"conflict\":");
    out.push_str(if rec.verdict.conflict {
        "true"
    } else {
        "false"
    });
    out.push_str(",\"decided\":");
    out.push_str(if rec.verdict.is_decided() {
        "true"
    } else {
        "false"
    });
    out.push_str(",\"exit\":");
    out.push_str(&rec.verdict.exit_code().to_string());
    out.push('}');

    out.push_str(",\"facts\":{");
    let mut first = true;
    for (id, cell) in rec.facts.iter() {
        if cell.val.is_unknown() {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&escape(id.name()));
        out.push(':');
        out.push_str(&escape(&cell.val.glyph().to_string()));
    }
    out.push('}');

    out.push_str(",\"measures\":{");
    let mut first = true;
    for (id, cell) in rec.facts.iter() {
        if cell.num.is_nan() {
            continue;
        }
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&escape(id.name()));
        out.push(':');
        out.push_str(&format!("{:.4}", cell.num));
    }
    out.push('}');

    out.push_str(",\"hits\":[");
    let mut first = true;
    for hit in rec.hits.iter().filter(|h| !h.value.is_false()) {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str("{\"rule\":");
        out.push_str(&escape(&hit.name));
        out.push_str(",\"severity\":");
        out.push_str(&escape(hit.severity.name()));
        out.push_str(",\"value\":");
        out.push_str(&escape(&hit.value.glyph().to_string()));
        out.push_str(",\"applicable\":");
        out.push_str(if hit.applicable { "true" } else { "false" });
        if let Some(region) = &hit.region {
            out.push_str(",\"region\":");
            out.push_str(&escape(region));
        }
        out.push('}');
    }
    out.push(']');

    out.push_str(",\"judgments\":[");
    let mut first = true;
    for judgment in rec
        .verdict
        .judgments
        .iter()
        .filter(|j| j.severity != t3_core::Grade::Clean)
    {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str("{\"id\":");
        out.push_str(&escape(judgment.id));
        out.push_str(",\"severity\":");
        out.push_str(&escape(judgment.severity.name()));
        out.push_str(",\"lo\":");
        out.push_str(&escape(&judgment.interval.lo.glyph().to_string()));
        out.push_str(",\"hi\":");
        out.push_str(&escape(&judgment.interval.hi.glyph().to_string()));
        out.push_str(",\"four\":");
        out.push_str(&escape(&judgment.four.glyph().to_string()));
        out.push('}');
    }
    out.push_str("]}");
    out
}

/// Serialises a skipped-path notice.
#[must_use]
pub fn error_line(message: &str) -> String {
    format!(
        "{{\"schema\":{},\"error\":{},\"skipped\":true}}",
        SCHEMA_VERSION,
        escape(message)
    )
}

#[cfg(test)]
mod tests {
    use super::{error_line, escape};

    #[test]
    fn escapes_the_awkward_characters() {
        assert_eq!(escape("a\"b"), "\"a\\\"b\"");
        assert_eq!(escape("a\\b"), "\"a\\\\b\"");
        assert_eq!(escape("a\nb"), "\"a\\nb\"");
        assert_eq!(escape("\u{1}"), "\"\\u0001\"");
        // Non-ASCII passes through as UTF-8, which JSON permits.
        assert_eq!(escape("加壳"), "\"加壳\"");
    }

    #[test]
    fn windows_paths_survive() {
        let json = escape(r"C:\Windows\System32\a.dll");
        assert!(json.contains(r"\\Windows\\"));
        assert!(!json.contains('\u{0}'));
    }

    #[test]
    fn error_line_is_valid_json_shaped() {
        let line = error_line("boom");
        assert!(line.starts_with("{\"schema\":1"));
        assert!(line.contains("\"skipped\":true"));
    }
}
