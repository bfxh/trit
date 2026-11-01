//! Human-readable output.
//!
//! Two audiences. The scan table is for triage at scale: one line per file, stable
//! width, the qualifier before the path so a `sort` on the column groups verdicts.
//! The explain view is for a single file and is built around the question an analyst
//! actually asks after "undecided": *what would settle it?*

use std::time::Duration;

use t3_core::{FactId, Facts, Grade, Verdict};
use t3_sig::Scan;

use crate::scan::Record;

fn paint(text: &str, code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn grade_paint(grade: Grade, text: &str, color: bool) -> String {
    let code = match grade {
        Grade::Clean => "32",
        Grade::Suspicious => "33",
        Grade::Malicious => "31",
    };
    paint(text, code, color)
}

/// Formats a byte count the way a person reads it.
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Formats an elapsed time for reports.
#[must_use]
pub fn human_duration(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0} ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{secs:.2} s")
    } else {
        format!("{:.0}m{:.0}s", (secs / 60.0).floor(), secs % 60.0)
    }
}

fn label_for(verdict: &Verdict) -> &'static str {
    if verdict.conflict {
        "conflict"
    } else if verdict.is_decided() {
        verdict.hi.name()
    } else {
        "undecided"
    }
}

/// One line of the scan table.
#[must_use]
pub fn line(record: &Record, color: bool) -> String {
    let label = label_for(&record.verdict);
    let padded = format!("{label:<10}");
    let head = if record.verdict.conflict {
        paint(&padded, "35", color)
    } else if !record.verdict.is_decided() {
        paint(&padded, "36", color)
    } else {
        grade_paint(record.verdict.hi, &padded, color)
    };

    let mut path = record.path.clone();
    if path.chars().count() > 56 {
        let tail: String = path.chars().skip(path.chars().count() - 53).collect();
        path = format!("...{tail}");
    }

    let mut notes = Vec::new();
    for judgment in &record.verdict.judgments {
        if judgment.severity == Grade::Clean || !judgment.applicable {
            continue;
        }
        if judgment.is_established() || judgment.is_open() {
            notes.push(format!("{}={}", judgment.id, judgment.interval.hi.glyph()));
        }
    }
    let detail = if notes.is_empty() {
        String::new()
    } else {
        format!("  {}", notes.join(" "))
    };
    format!("{head} {path:<56}{detail}")
}

/// End-of-run summary.
pub fn summary(
    records: &[Record],
    problems: &[String],
    bytes: u64,
    elapsed: Duration,
    tally: &[usize; 5],
) {
    let secs = elapsed.as_secs_f64().max(1e-9);
    let files = records.len();
    println!();
    println!(
        "扫描 {} 个文件 · {} · {} · {:.2} GiB/s · {:.0} 文件/s",
        files,
        human_bytes(bytes),
        human_duration(elapsed),
        bytes as f64 / secs / (1024.0 * 1024.0 * 1024.0),
        files as f64 / secs
    );
    println!(
        "确定干净 {} · 可疑 {} · 恶意 {} · 未定 {} · 冲突 {}{}",
        tally[0],
        tally[1],
        tally[2],
        tally[3],
        tally[4],
        if problems.is_empty() {
            String::new()
        } else {
            format!(" · 跳过 {}", problems.len())
        }
    );
}

/// One row of the section table shown by `explain`.
///
/// The table is here because "which section is high-entropy" and "which section is
/// writable and executable" are the two questions an analyst asks immediately after
/// seeing a structural finding, and neither is answerable from a verdict alone.
pub struct SectionRow {
    /// Section name.
    pub name: String,
    /// Virtual address, or file offset for ELF.
    pub va: u64,
    /// Raw size on disk.
    pub raw: u64,
    /// Declared virtual size.
    pub vsize: u64,
    /// Shannon entropy of the raw bytes, when they are present.
    pub entropy: Option<f64>,
    /// Permission letters, always three characters.
    pub flags: String,
}

fn fact_brief(facts: &Facts, id: FactId) -> String {
    let cell = facts.cell(id);
    if cell.num.is_nan() {
        format!("{}={}", id.name(), cell.val.glyph())
    } else {
        format!("{}={}({:.3})", id.name(), cell.val.glyph(), cell.num)
    }
}

/// The single-file view.
#[allow(
    clippy::too_many_arguments,
    reason = "单文件报告就是这 8 个输入；打包成结构体只是把参数搬运一遍，调用点会更难读"
)]
pub fn explain(
    path: &str,
    size: u64,
    kind: t3_format::Kind,
    note: &str,
    facts: &Facts,
    verdict: &Verdict,
    scan: Option<&Scan>,
    sections: &[SectionRow],
) {
    println!("文件      {path}");
    println!(
        "容器      {} · {} 字节 · {}",
        kind.name(),
        size,
        human_bytes(size)
    );
    if kind == t3_format::Kind::Unknown {
        println!("备注      {note}");
    }

    let headline = label_for(verdict);
    let colored = if verdict.conflict {
        paint(headline, "35", true)
    } else if !verdict.is_decided() {
        paint(headline, "36", true)
    } else {
        grade_paint(verdict.hi, headline, true)
    };
    println!("判定      {colored} · {}", verdict.summary());
    println!("区间      [{} , {}]", verdict.lo.name(), verdict.hi.name());
    println!(
        "冲突      {}{}",
        if verdict.conflict { "是" } else { "否" },
        if verdict.conflict {
            "（同一假设的正反两面都有证据）"
        } else {
            ""
        }
    );
    let decided = facts.decided_count();
    println!("证据      {decided}/{} 条已决", FactId::COUNT);

    if !sections.is_empty() {
        println!();
        println!(
            "区段      {:<12}{:>10}{:>10}{:>10}{:>7}  属性",
            "名称", "VA", "原始大小", "虚拟大小", "熵"
        );
        for row in sections {
            let entropy = match row.entropy {
                Some(e) => format!("{e:.2}"),
                None => "-".to_string(),
            };
            println!(
                "          {:<12}{:>10}{:>10}{:>10}{:>7}  {}",
                row.name,
                format!("{:#x}", row.va),
                format!("{:#x}", row.raw),
                format!("{:#x}", row.vsize),
                entropy,
                row.flags
            );
        }
    }

    let applicable = verdict.judgments.iter().filter(|j| j.applicable).count();
    println!();
    println!(
        "假设      {} 条（{} 条适用于该容器）",
        verdict.judgments.len(),
        applicable
    );
    for judgment in verdict.judgments.iter().filter(|j| j.applicable) {
        let glyph = judgment.interval.hi.glyph();
        let mark = if judgment.interval.is_decided() {
            glyph
        } else {
            // An undecided interval is shown with both ends so the width is visible.
            'U'
        };
        let line = format!("  {mark}  {:<22} — {}", judgment.id, judgment.title);
        println!("{line}");

        if judgment.is_established() {
            let supporting: Vec<String> = judgment
                .undecided
                .is_empty()
                .then(|| {
                    judgment
                        .four
                        .is_conflict()
                        .then_some("（同时存在良性解释，见冲突标记）".to_string())
                })
                .flatten()
                .into_iter()
                .collect();
            let suffix = if supporting.is_empty() {
                String::new()
            } else {
                supporting.join("")
            };
            println!("       ✓ 成立{suffix}");
        } else if judgment.is_open() {
            let mut parts: Vec<String> = judgment
                .undecided
                .iter()
                .map(|id| fact_brief(facts, *id))
                .collect();
            if parts.is_empty() {
                parts.push("（区间未收窄）".to_string());
            }
            println!("       ? {}", parts.join(" "));
        }
    }

    // Counterfactuals: the part that answers "what would change your mind".
    let mut counterfactuals: Vec<(String, &'static str, Grade)> = Vec::new();
    for judgment in verdict.judgments.iter().filter(|j| j.applicable) {
        for id in &judgment.would_establish {
            counterfactuals.push((
                format!("{} 若为真", fact_brief(facts, *id)),
                judgment.id,
                judgment.severity,
            ));
        }
    }
    if !counterfactuals.is_empty() {
        println!();
        println!("反事实    哪些未决事实足以改变结论");
        for (fact, hypothesis, severity) in counterfactuals {
            // Only claim a grade change when one would actually happen. Saying "rises to
            // suspicious" on a file that is already suspicious is a false statement
            // about the tool's own output.
            if severity > verdict.hi {
                println!(
                    "  {:<44} → {} 成立，判定升至 {}",
                    fact,
                    hypothesis,
                    severity.name()
                );
            } else {
                println!(
                    "  {:<44} → {} 成立（当前判定已不低于 {}）",
                    fact,
                    hypothesis,
                    severity.name()
                );
            }
        }
    }

    if let Some(scan) = scan {
        let matched: Vec<_> = scan.matched().collect();
        let undecided: Vec<_> = scan.undecided().collect();
        println!();
        println!(
            "签名      {} 条规则 · {} 命中 · {} 未决",
            scan.hits.len(),
            matched.len(),
            undecided.len()
        );
        for hit in &matched {
            println!(
                "  ✓ {:<26} {:<7} {}",
                hit.name,
                hit.severity.name(),
                hit.region.clone().unwrap_or_default()
            );
        }
        for hit in &undecided {
            println!(
                "  ? {:<26} {:<7} 作用域字节缺失，无法判定",
                hit.name,
                hit.severity.name()
            );
        }
    }

    // Measurements, so the reader can check the calibration instead of trusting it.
    println!();
    println!("测量");
    for (id, cell) in facts.iter() {
        if !cell.num.is_nan() {
            println!("  {:<28} {:.4}", id.name(), cell.num);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{human_bytes, human_duration};
    use std::time::Duration;

    #[test]
    fn byte_formatting() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.00 KiB");
        assert_eq!(human_bytes(1536), "1.50 KiB");
        assert_eq!(human_bytes(10 * 1024 * 1024), "10.00 MiB");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(human_duration(Duration::from_millis(250)), "250 ms");
        assert_eq!(human_duration(Duration::from_millis(2500)), "2.50 s");
        assert_eq!(human_duration(Duration::from_secs(125)), "2m5s");
    }
}
