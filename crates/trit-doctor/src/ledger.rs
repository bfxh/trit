//! `docs/ERRORS.md` 台账面：固化列里的断言号必须真的存在，反向也不许漂移。

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::{Face, Status};

static ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z]{1,2}[0-9]{1,2}[a-z]?").expect("static id regex"));
static CID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"C[0-9]{3}").expect("static cid regex"));
static LREF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"L[0-9]{2,3}").expect("static ledger ref regex"));
static GROUP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z]{1,2}\s*组").expect("static group regex"));

pub const BACKLOG_KW: [&str; 3] = ["欠账", "待办", "未固化"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub lid: String,
    pub fix: String,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DRow {
    pub lid: String,
    pub cells: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    pub entries: Vec<Entry>,
    pub d_rows: Vec<DRow>,
}

#[must_use]
pub fn parse_ledger(text: &str) -> Ledger {
    let mut ledger = Ledger::default();
    let mut section = None;
    for line in text.lines() {
        if let Some(rest) = line.trim().strip_prefix("## ") {
            let bytes = rest.as_bytes();
            if bytes.len() >= 2 && bytes[0].is_ascii_uppercase() && bytes[1] == b'.' {
                section = Some(bytes[0] as char);
                continue;
            }
        }
        if !line.starts_with('|') {
            continue;
        }
        let rest = line[1..].trim_start();
        if !rest.starts_with('L') {
            continue;
        }
        let digits: String = rest[1..].chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() {
            continue;
        }
        let after_digits = &rest[1 + digits.len()..];
        let after_digits = after_digits.trim_start();
        if !after_digits.starts_with('|') {
            continue;
        }
        let lid = format!("L{digits}");
        let mut cells: Vec<String> = line
            .replace("\\|", "\0")
            .split('|')
            .map(|cell| cell.trim().replace('\0', "|"))
            .collect();
        if cells.first().is_some_and(String::is_empty) {
            cells.remove(0);
        }
        if cells.last().is_some_and(String::is_empty) {
            cells.pop();
        }
        if section == Some('D') {
            ledger.d_rows.push(DRow { lid, cells });
        } else {
            let fix = if cells.len() >= 5 {
                cells.last().cloned().unwrap_or_default()
            } else {
                String::new()
            };
            ledger.entries.push(Entry { lid, fix, cells });
        }
    }
    ledger
}

fn boundary_ok(text: &str, start: usize, end: usize) -> bool {
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let is_ascii_alnum = |ch: char| ch.is_ascii_alphanumeric();
    before.is_none_or(|ch| !is_ascii_alnum(ch)) && after.is_none_or(|ch| !is_ascii_alnum(ch))
}

fn matches(re: &Regex, text: &str) -> Vec<String> {
    re.find_iter(text)
        .filter(|m| boundary_ok(text, m.start(), m.end()))
        .map(|m| m.as_str().to_string())
        .collect()
}

#[must_use]
pub fn ids_in(text: &str) -> Vec<String> {
    matches(&ID_RE, text)
}

#[must_use]
pub fn cids_in(text: &str) -> Vec<String> {
    matches(&CID_RE, text)
}

#[must_use]
pub fn ledger_refs_in(text: &str) -> Vec<String> {
    LREF_RE
        .find_iter(text)
        .filter(|m| {
            let before = text[..m.start()].chars().next_back();
            let after = text[m.end()..].chars().next();
            // Python: (?<![A-Za-z0-9])(L\d{2,3})(?![0-9]) -- 尾部只挡数字，不挡字母。
            before.is_none_or(|ch| !ch.is_ascii_alphanumeric())
                && after.is_none_or(|ch| !ch.is_ascii_digit())
        })
        .map(|m| m.as_str().to_string())
        .collect()
}

#[must_use]
pub fn id_exists(token: &str, selftest_text: &str) -> bool {
    selftest_text.match_indices(token).any(|(start, _)| {
        let end = start + token.len();
        boundary_ok(selftest_text, start, end)
    })
}

fn group_only(fix: &str) -> bool {
    GROUP_RE.find_iter(fix).any(|m| {
        boundary_ok(fix, m.start(), m.end())
            && fix[m.start()..]
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_uppercase())
    })
}

#[must_use]
pub fn face_ledger(
    ledger: Option<&Ledger>,
    selftest_text: Option<&str>,
    registry_ids: Option<&HashSet<String>>,
) -> Face {
    let Some(ledger) = ledger else {
        return Face::new(
            "ledger",
            Status::Unavailable,
            "docs/ERRORS.md",
            "missing or no parseable rows (truncated?)".to_string(),
        );
    };
    let Some(selftest_text) = selftest_text else {
        return Face::new(
            "ledger",
            Status::Unavailable,
            "tools/selftest.ps1",
            "selftest source missing; claimed assertion ids unverifiable".to_string(),
        );
    };

    let mut all_ids: HashSet<String> = ledger.d_rows.iter().map(|row| row.lid.clone()).collect();
    for entry in &ledger.entries {
        all_ids.insert(entry.lid.clone());
    }

    let mut ghost: Vec<(String, Vec<String>)> = Vec::new();
    let mut unpinned: Vec<String> = Vec::new();
    let mut group_only_ids: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    let mut unverified_c = 0usize;

    for entry in &ledger.entries {
        let toks = ids_in(&entry.fix);
        let cids = cids_in(&entry.fix);
        let mut bad: Vec<String> = toks
            .iter()
            .filter(|token| !id_exists(token, selftest_text))
            .cloned()
            .collect();
        if let Some(ids) = registry_ids {
            bad.extend(
                cids.iter()
                    .filter(|cid| !ids.contains(cid.as_str()))
                    .cloned(),
            );
        }
        let verified = toks.iter().any(|token| id_exists(token, selftest_text))
            || registry_ids.is_some_and(|ids| cids.iter().any(|cid| ids.contains(cid)));
        if registry_ids.is_none() {
            unverified_c += cids.len();
        }
        if !bad.is_empty() {
            ghost.push((entry.lid.clone(), bad));
        } else if verified {
            pinned += 1;
        } else {
            unpinned.push(entry.lid.clone());
            if group_only(&entry.fix) {
                group_only_ids.push(entry.lid.clone());
            }
        }
    }

    let refs = ledger_refs_in(selftest_text);
    let mut reverse_missing: Vec<String> = refs
        .into_iter()
        .filter(|lid| !all_ids.contains(lid))
        .collect();
    reverse_missing.sort();
    reverse_missing.dedup();

    let status = if !ghost.is_empty() || !reverse_missing.is_empty() {
        Status::Fail
    } else if !unpinned.is_empty() {
        Status::Warn
    } else {
        Status::Ok
    };

    let mut detail = format!(
        "entries={} pinned={} unpinned={} ghost={} group_only={} reverse_missing={}",
        ledger.entries.len(),
        pinned,
        unpinned.len(),
        ghost.len(),
        group_only_ids.len(),
        reverse_missing.len()
    );
    if registry_ids.is_none() {
        detail.push_str(&format!(
            " registry_unavailable_c_ids_unverified={unverified_c}"
        ));
    }
    if !ghost.is_empty() {
        let rendered: Vec<String> = ghost
            .iter()
            .take(8)
            .map(|(lid, bad)| format!("{lid}:{}", bad.join(",")))
            .collect();
        detail.push_str(" ghost=");
        detail.push_str(&rendered.join(";"));
    }
    if !unpinned.is_empty() {
        detail.push_str(" gap_first10=");
        detail.push_str(
            &unpinned
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    if !reverse_missing.is_empty() {
        detail.push_str(" reverse=");
        detail.push_str(
            &reverse_missing
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let evidence = format!(
        "docs/ERRORS.md rows={} vs tools/selftest.ps1",
        ledger.entries.len()
    );
    Face::new("ledger", status, &evidence, detail)
}

#[must_use]
pub fn face_backlog(ledger: Option<&Ledger>) -> Face {
    let Some(ledger) = ledger else {
        return Face::new(
            "backlog",
            Status::Unavailable,
            "docs/ERRORS.md",
            "missing or no parseable rows (truncated?)".to_string(),
        );
    };
    let d_ids: Vec<&str> = ledger.d_rows.iter().map(|row| row.lid.as_str()).collect();
    let keyword: Vec<&str> = ledger
        .entries
        .iter()
        .filter(|entry| {
            let joined = entry.cells.join(" ");
            BACKLOG_KW.iter().any(|kw| joined.contains(kw))
        })
        .map(|entry| entry.lid.as_str())
        .collect();
    let total: Vec<&str> = d_ids.iter().chain(keyword.iter()).copied().collect();
    let mut detail = format!(
        "backlog={} d_section={} keyword={}",
        total.len(),
        d_ids.len(),
        keyword.len()
    );
    if !total.is_empty() {
        detail.push_str(" ids=");
        detail.push_str(&total.iter().take(10).copied().collect::<Vec<_>>().join(","));
    }
    let status = if total.is_empty() {
        Status::Ok
    } else {
        Status::Warn
    };
    Face::new("backlog", status, "docs/ERRORS.md", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
| L01 | 问题 | 根因 | 修复 | AI30 |\n\
| L02 | 问题 | 根因 | 修复 |  |\n\
| L03 | 问题 | 根因 | 修复 | G5 |\n\
\n\
## D. 欠账\n\
\n\
| L90 | 欠账 | 待办 |\n";

    #[test]
    fn parses_rows_and_section_d() {
        let ledger = parse_ledger(SAMPLE);
        assert_eq!(ledger.entries.len(), 3);
        assert_eq!(ledger.entries[0].lid, "L01");
        assert_eq!(ledger.entries[0].fix, "AI30");
        assert_eq!(ledger.entries[1].fix, "");
        assert_eq!(ledger.entries[2].fix, "G5");
        assert_eq!(ledger.d_rows.len(), 1);
        assert_eq!(ledger.d_rows[0].lid, "L90");
    }

    #[test]
    fn ids_need_word_boundaries() {
        assert_eq!(ids_in("AI30, G5, AM25, C901"), vec!["AI30", "G5", "AM25"]);
        assert!(!id_exists("G5", "AG5"));
        assert!(id_exists("G5", "x G5,"));
    }

    #[test]
    fn ghost_and_reverse_drift_are_fail() {
        let ledger = parse_ledger("| L01 | x | y | z | AI30 |\n| L02 | x | y | z | Z9 |\n");
        let face = face_ledger(Some(&ledger), Some("AI30 L03"), None);
        assert_eq!(face.status, Status::Fail);
        assert!(face.detail.contains("ghost=L02:Z9"));
        assert!(face.detail.contains("reverse=L03"));
    }

    #[test]
    fn no_pin_is_warn_not_fail() {
        let ledger = parse_ledger("| L01 | x | y | z |  |\n");
        let face = face_ledger(Some(&ledger), Some("AI30"), None);
        assert_eq!(face.status, Status::Warn);
        assert!(face.detail.contains("unpinned=1"));
    }

    #[test]
    fn backlog_reads_section_d_and_keywords() {
        let ledger = parse_ledger("| L01 | 欠账 |\n\n## D. 欠账\n\n| L90 | x |\n");
        let face = face_backlog(Some(&ledger));
        assert_eq!(face.status, Status::Warn);
        assert!(face.detail.contains("backlog=2 d_section=1 keyword=1"));
    }
}
