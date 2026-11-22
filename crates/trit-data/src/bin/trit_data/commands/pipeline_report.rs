//! 管线报告与数据卡：决策分布、原因、许可证/语言/覆盖率、去重与依赖统计。

use std::collections::BTreeMap;

use trit_data::gate::Verdict;
use trit_data::repo;
use trit_data::repo::Unit;
use trit_learn::json::Json;
use trit_learn::manifest::Fingerprint;

pub struct VerdictRow<'a> {
    pub decision: &'a str,
    pub reasons: &'a [&'static str],
}

pub fn unit_line(u: &Unit, v: &Verdict) -> String {
    Json::obj()
        .put("path", u.path.clone())
        .put("lang", u.lang.clone())
        .put("bytes", u.bytes)
        .put("decision", v.decision.as_str().to_string())
        .put("reasons", strings(&v.reasons))
        .put(
            "license",
            u.license.clone().unwrap_or_else(|| "missing".into()),
        )
        .put("generated", u.generated)
        .put("is_test", u.is_test)
        .put("tests", strings(&u.tests))
        .put("imports_resolved", strings(&u.imports_resolved))
        .dump()
}

pub fn report(
    root: &str,
    rows: &[VerdictRow<'_>],
    total: usize,
    d: &repo::deps::Summary,
    dup: &repo::dup::Dup,
) -> Json {
    let (decisions, reasons) = tally(rows);
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-pipeline")
        .put("repo", root.to_string())
        .put("files", total as u64)
        .put("decisions", map_json(&decisions))
        .put("rates", rates_json(&decisions, total))
        .put("reasons", map_json(&reasons))
        .put("deps", super::repo_stats::summary_json(d))
        .put("duplicates", super::repo_dup::json(dup))
}

pub fn card(
    root: &str,
    units: &[Unit],
    rows: &[VerdictRow<'_>],
    d: &repo::deps::Summary,
    dup: &repo::dup::Dup,
) -> Json {
    let mut licenses: BTreeMap<String, usize> = BTreeMap::new();
    let mut languages: BTreeMap<String, usize> = BTreeMap::new();
    let (mut licensed, mut tested, mut resolved, mut generated) = (0usize, 0usize, 0usize, 0usize);
    for u in units {
        *licenses
            .entry(u.license.clone().unwrap_or_else(|| "missing".into()))
            .or_insert(0) += 1;
        *languages.entry(u.lang.clone()).or_insert(0) += 1;
        licensed += usize::from(u.license.is_some());
        tested += usize::from(!u.is_test && !u.tests.is_empty());
        resolved += usize::from(!u.imports_resolved.is_empty());
        generated += usize::from(u.generated);
    }
    let n = units.len().max(1) as f64;
    let (decisions, _) = tally(rows);
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-pipeline-card")
        .put("repo", root.to_string())
        .put("records", units.len() as u64)
        .put("sources", Json::obj().put(source_name(root), units.len() as u64))
        .put("licenses", map_json(&licenses))
        .put("languages", map_json(&languages))
        .put("generated", generated as u64)
        .put("decisions", map_json(&decisions))
        .put(
            "coverage",
            Json::obj()
                .put("license", licensed as f64 / n)
                .put("tests", tested as f64 / n)
                .put("imports_resolved", resolved as f64 / n),
        )
        .put("deps", super::repo_stats::summary_json(d))
        .put("duplicates", super::repo_dup::json(dup))
}

pub fn manifest_json(f: &Fingerprint) -> Json {
    Json::obj()
        .put("kind", "trit-data-manifest")
        .put("hash", f.hash.clone())
        .put("files", f.files as u64)
        .put("bytes", f.bytes)
}

fn tally(rows: &[VerdictRow<'_>]) -> (BTreeMap<String, usize>, BTreeMap<String, usize>) {
    let mut decisions = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    for r in rows {
        *decisions.entry(r.decision.to_string()).or_insert(0) += 1;
        for why in r.reasons {
            *reasons.entry((*why).to_string()).or_insert(0) += 1;
        }
    }
    (decisions, reasons)
}

fn rates_json(decisions: &BTreeMap<String, usize>, total: usize) -> Json {
    let n = total.max(1) as f64;
    let mut out = Json::obj();
    for k in ["accept", "defer", "reject"] {
        let c = decisions.get(k).copied().unwrap_or(0) as f64;
        out = out.put(k, c / n);
    }
    out
}

fn map_json(m: &BTreeMap<String, usize>) -> Json {
    let mut out = Json::obj();
    for (k, v) in m {
        out = out.put(k.clone(), *v as u64);
    }
    out
}

fn strings(v: &[impl AsRef<str>]) -> Json {
    Json::arr(v.iter().map(|s| Json::str(s.as_ref())).collect())
}

fn source_name(root: &str) -> String {
    std::path::Path::new(root)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(root)
        .to_string()
}
