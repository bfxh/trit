//! 依赖图：import → 仓库内文件边、测试文件匹配、依赖闭包。
//!
//! 解析是确定性的（路径归一化 + 固定候选顺序）；解析不到的 import 记入
//! `unresolved`（外部依赖也在其中），依赖闭合率因此可复算。

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::paths::{JS_EXTS, file_name, norm, parent_dir, targets, up};
use crate::repo::Unit;
use super::kind::{self, Kind};

pub use super::deps_report::{Summary, summarize};

const JVM_EXTS: &[&str] = &["java", "kt", "scala"];

#[derive(Default)]
struct Link {
    resolved: Vec<String>,
    unresolved: Vec<String>,
    external: Vec<String>,
}

type Links = Vec<Link>;
type Tests = Vec<(Vec<String>, Vec<String>)>;

/// 填 `imports_resolved` / `unresolved` / `imports_external` / 测试关联。
pub fn annotate(units: &mut [Unit]) {
    let (links, tests) = plan(units);
    for (i, u) in units.iter_mut().enumerate() {
        u.imports_resolved = links[i].resolved.clone();
        u.unresolved = links[i].unresolved.clone();
        u.imports_external = links[i].external.clone();
        u.tests = tests[i].0.clone();
        u.tests_for = tests[i].1.clone();
    }
}

/// 起点容错：接受工作区相对路径（`crates/x/src/lib.rs`）或仓库相对路径。
fn match_start(units: &[Unit], start: &str) -> String {
    if units.iter().any(|u| u.path == start) {
        return start.to_string();
    }
    units
        .iter()
        .filter(|u| start.ends_with(&format!("/{}", u.path)))
        .max_by_key(|u| u.path.len())
        .map(|u| u.path.clone())
        .unwrap_or_else(|| start.to_string())
}

fn plan(units: &[Unit]) -> (Links, Tests) {
    let paths: BTreeSet<String> = units.iter().map(|u| u.path.clone()).collect();
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &paths {
        by_name
            .entry(file_name(p).to_string())
            .or_default()
            .push(p.clone());
    }
    let links = units
        .iter()
        .map(|u| {
            let mut link = Link::default();
            for imp in &u.imports {
                match resolve(&u.path, &u.lang, imp, &paths, &by_name) {
                    Some(p) if p != u.path => {
                        if !link.resolved.contains(&p) {
                            link.resolved.push(p);
                        }
                    }
                    Some(_) => {}
                    None => match kind::classify(&u.lang, imp, &paths) {
                        Kind::External => link.external.push(imp.clone()),
                        Kind::Unresolved => link.unresolved.push(imp.clone()),
                    },
                }
            }
            link
        })
        .collect();
    (links, link_tests(units, &paths))
}
fn link_tests(units: &[Unit], paths: &BTreeSet<String>) -> Tests {
    let pos: BTreeMap<&str, usize> = units
        .iter()
        .enumerate()
        .map(|(i, u)| (u.path.as_str(), i))
        .collect();
    let mut out = vec![(Vec::new(), Vec::new()); units.len()];
    for (i, u) in units.iter().enumerate() {
        if !u.is_test {
            continue;
        }
        for t in targets(u, paths) {
            if let Some(&j) = pos.get(t.as_str()) {
                if !out[j].0.contains(&u.path) {
                    out[j].0.push(u.path.clone());
                }
                if !out[i].1.contains(&t) {
                    out[i].1.push(t);
                }
            }
        }
    }
    for v in &mut out {
        v.0.sort();
        v.1.sort();
    }
    out
}

fn resolve(
    from: &str,
    lang: &str,
    imp: &str,
    paths: &BTreeSet<String>,
    by_name: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let dir = parent_dir(from);
    match lang {
        "javascript" | "typescript" => resolve_rel(&dir, imp, JS_EXTS, paths),
        "python" => resolve_py(&dir, imp, paths),
        "rust" => resolve_rs(from, &dir, imp, paths),
        "go" => resolve_go(imp, paths),
        "java" | "kotlin" | "scala" => resolve_jvm(imp, paths),
        "c" | "cpp" | "cuda" | "opencl" => resolve_include(&dir, imp, paths, by_name),
        _ => None,
    }
}

fn resolve_rel(dir: &str, imp: &str, exts: &[&str], paths: &BTreeSet<String>) -> Option<String> {
    if !imp.starts_with('.') {
        return None;
    }
    let base = norm(&format!("{dir}/{imp}"));
    let mut cands = vec![base.clone()];
    if let Some(stem) = base.strip_suffix(".js") {
        cands.push(format!("{stem}.ts"));
        cands.push(format!("{stem}.tsx"));
    }
    for e in exts {
        cands.push(format!("{base}.{e}"));
        cands.push(format!("{base}/index.{e}"));
    }
    cands.into_iter().find(|c| paths.contains(c))
}

fn resolve_py(dir: &str, imp: &str, paths: &BTreeSet<String>) -> Option<String> {
    let dots = imp.chars().take_while(|c| *c == '.').count();
    let rel: String = imp[dots..]
        .trim()
        .split('.')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() {
        return None;
    }
    let base = if dots == 0 {
        String::new()
    } else {
        up(dir, dots - 1)
    };
    let mut cands = Vec::new();
    for root in [base.as_str(), "", dir] {
        let p = if root.is_empty() {
            rel.clone()
        } else {
            format!("{root}/{rel}")
        };
        cands.push(format!("{p}.py"));
        cands.push(format!("{p}/__init__.py"));
    }
    cands.into_iter().find(|c| paths.contains(c))
}

fn resolve_rs(from: &str, dir: &str, imp: &str, paths: &BTreeSet<String>) -> Option<String> {
    let prefix = from.find("src/").map(|i| &from[..i]).unwrap_or("");
    if imp.contains('{') || imp.contains('}') || imp.contains('*') {
        return None;
    }
    let (base, rest) = if let Some(r) = imp.strip_prefix("crate::") {
        (String::new(), r)
    } else if let Some(r) = imp.strip_prefix("self::") {
        (dir.to_string(), r)
    } else if let Some(r) = imp.strip_prefix("super::") {
        (up(dir, 1), r)
    } else if imp.contains("::") {
        return None;
    } else {
        (dir.to_string(), imp)
    };
    let rel: Vec<&str> = rest.split("::").filter(|s| !s.is_empty()).collect();
    if rel.is_empty() {
        return None;
    }
    for n in (1..=rel.len()).rev() {
        let sub = rel[..n].join("/");
        let (a, b) = if base.is_empty() {
            (
                format!("{prefix}src/{sub}.rs"),
                format!("{prefix}src/{sub}/mod.rs"),
            )
        } else {
            (format!("{base}/{sub}.rs"), format!("{base}/{sub}/mod.rs"))
        };
        if let Some(c) = [a, b].into_iter().find(|c| paths.contains(c)) {
            return Some(c);
        }
    }
    None
}

fn resolve_go(imp: &str, paths: &BTreeSet<String>) -> Option<String> {
    let imp = imp.trim_matches('"').trim();
    if imp.is_empty() {
        return None;
    }
    let (suffix, head) = (format!("/{imp}/"), format!("{imp}/"));
    paths
        .iter()
        .find(|p| p.ends_with(".go") && (p.contains(&suffix) || p.starts_with(&head)))
        .cloned()
}

fn resolve_jvm(imp: &str, paths: &BTreeSet<String>) -> Option<String> {
    let rel = imp.replace('.', "/");
    for e in JVM_EXTS {
        let head = format!("{rel}.{e}");
        let tail = format!("/{head}");
        if let Some(p) = paths
            .iter()
            .find(|p| p.as_str() == head || p.ends_with(&tail))
        {
            return Some(p.clone());
        }
    }
    None
}

fn resolve_include(
    dir: &str,
    imp: &str,
    paths: &BTreeSet<String>,
    by_name: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    let direct = norm(&format!("{dir}/{imp}"));
    if paths.contains(&direct) {
        return Some(direct);
    }
    if paths.contains(imp) {
        return Some(imp.to_string());
    }
    by_name
        .get(file_name(imp))
        .filter(|v| v.len() == 1)
        .map(|v| v[0].clone())
}

/// 从 `start` 沿依赖方向展开的闭包（含起点），最多 `max` 个文件。
pub fn closure(units: &[Unit], start: &str, max: usize) -> Vec<String> {
    let start = match_start(units, start);
    let map: BTreeMap<&str, &[String]> = units
        .iter()
        .map(|u| (u.path.as_str(), u.imports_resolved.as_slice()))
        .collect();
    let limit = max.max(1);
    let mut seen = BTreeSet::new();
    let mut q = VecDeque::from([start]);
    while let Some(p) = q.pop_front() {
        if seen.len() >= limit {
            break;
        }
        if !seen.insert(p.clone()) {
            continue;
        }
        if let Some(next) = map.get(p.as_str()) {
            for n in *next {
                if !seen.contains(n) {
                    q.push_back(n.clone());
                }
            }
        }
    }
    seen.into_iter().collect()
}
