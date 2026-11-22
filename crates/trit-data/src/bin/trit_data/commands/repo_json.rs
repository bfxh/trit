//! repo 输出：RepoUnit 行、依赖图文档、依赖闭包。

use std::collections::BTreeMap;

use trit_data::repo::{self, Unit};
use trit_learn::json::Json;
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
                .put("imports_external", strings(&u.imports_external))
                .put("imports_dynamic", strings(&u.imports_dynamic))
                .put("imports_resolved", strings(&u.imports_resolved))
                .put("imports_external", strings(&u.imports_external))
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
        .put("stats", super::repo_stats::summary_json(d))
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
