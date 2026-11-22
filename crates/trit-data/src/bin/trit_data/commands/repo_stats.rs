//! repo 统计与输出：报告、RepoUnit 行、依赖图、依赖闭包。

use std::collections::BTreeMap;

use trit_data::repo::{self, Unit};
use trit_learn::json::Json;

#[derive(Default)]
pub struct Stats {
    pub langs: BTreeMap<String, usize>,
    pub licenses: BTreeMap<String, usize>,
    pub generated: usize,
    pub tests: usize,
    pub with_imports: usize,
    pub license_missing: usize,
    pub imports_unknown: usize,
}

pub fn stats(units: &[Unit]) -> Stats {
    let mut s = Stats::default();
    for u in units {
        *s.langs.entry(u.lang.clone()).or_insert(0) += 1;
        *s.licenses
            .entry(u.license.clone().unwrap_or_else(|| "missing".into()))
            .or_insert(0) += 1;
        s.generated += usize::from(u.generated);
        s.tests += usize::from(u.is_test);
        s.with_imports += usize::from(!u.imports.is_empty());
        s.license_missing += usize::from(u.license.is_none());
        s.imports_unknown += usize::from(!u.imports_known);
    }
    s
}

pub fn report(root: &str, units: &[Unit], s: &Stats, d: &repo::deps::Summary) -> Json {
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-repo")
        .put("repo", root.to_string())
        .put("files", units.len() as u64)
        .put("languages", map_json(&s.langs))
        .put("licenses", map_json(&s.licenses))
        .put("generated", s.generated as u64)
        .put("tests", s.tests as u64)
        .put("with_imports", s.with_imports as u64)
        .put("license_missing", s.license_missing as u64)
        .put("imports_unknown", s.imports_unknown as u64)
        .put("tests_ratio", s.tests as f64 / units.len().max(1) as f64)
        .put("deps", summary_json(d))
}

fn summary_json(d: &repo::deps::Summary) -> Json {
    Json::obj()
        .put("edges", d.edges as u64)
        .put("imports_resolved", d.imports_resolved as u64)
        .put("imports_unresolved", d.imports_unresolved as u64)
        .put("tested_sources", d.tested_sources as u64)
        .put("test_links", d.test_links as u64)
        .put("cyclic", d.cyclic as u64)
}

pub fn map_json(m: &BTreeMap<String, usize>) -> Json {
    let mut out = Json::obj();
    for (k, v) in m {
        out = out.put(k.clone(), *v as u64);
    }
    out
}

pub fn units_lines(units: &[Unit]) -> Vec<String> {
    units
        .iter()
        .map(|u| {
            Json::obj()
                .put("repo", u.repo.clone())
                .put("path", u.path.clone())
                .put("lang", u.lang.clone())
                .put("bytes", u.bytes)
                .put("generated", u.generated)
                .put(
                    "license",
                    u.license.clone().unwrap_or_else(|| "missing".into()),
                )
                .put("imports", strings(&u.imports))
                .put("imports_known", u.imports_known)
                .put("imports_resolved", strings(&u.imports_resolved))
                .put("unresolved", strings(&u.unresolved))
                .put("tests", strings(&u.tests))
                .put("tests_for", strings(&u.tests_for))
                .put("is_test", u.is_test)
                .dump()
        })
        .collect()
}

pub fn deps_document(root: &str, units: &[Unit], d: &repo::deps::Summary) -> Json {
    let mut edges = Vec::new();
    for u in units {
        for t in &u.imports_resolved {
            edges.push(Json::obj().put("from", u.path.clone()).put("to", t.clone()));
        }
    }
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-deps")
        .put("repo", root.to_string())
        .put("nodes", Json::arr(units.iter().map(node_json).collect()))
        .put("edges", Json::arr(edges))
        .put("stats", summary_json(d))
}

fn node_json(u: &Unit) -> Json {
    Json::obj()
        .put("path", u.path.clone())
        .put("lang", u.lang.clone())
        .put("imports_resolved", strings(&u.imports_resolved))
        .put("unresolved", strings(&u.unresolved))
        .put("tests", strings(&u.tests))
}

pub fn closure_json(units: &[Unit], start: &str, max: usize) -> Json {
    let files = repo::deps::closure(units, start, max);
    let by: BTreeMap<&str, &Unit> = units.iter().map(|u| (u.path.as_str(), u)).collect();
    let arr = files
        .iter()
        .map(|p| {
            let mut o = Json::obj().put("path", p.clone());
            if let Some(u) = by.get(p.as_str()) {
                o = o.put("lang", u.lang.clone()).put("bytes", u.bytes);
            }
            o
        })
        .collect();
    Json::obj()
        .put("schema", 1u32)
        .put("kind", "trit-data-closure")
        .put("start", start.to_string())
        .put("max_files", max as u64)
        .put("files", Json::arr(arr))
}

fn strings(v: &[String]) -> Json {
    Json::arr(v.iter().map(Json::str).collect())
}
