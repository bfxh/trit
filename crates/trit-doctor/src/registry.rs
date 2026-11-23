//! `.agent/checks/registry.json` 面：只做存在性探测，不执行任何检查。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::{Face, Status};

#[derive(Debug, Clone)]
pub struct RegistryDoc {
    pub checks: Vec<Value>,
    pub ids: HashSet<String>,
}

#[must_use]
pub fn load(root: &Path) -> Option<RegistryDoc> {
    let text = fs::read_to_string(root.join(".agent").join("checks").join("registry.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let checks = match &value {
        Value::Object(map) => map.get("checks")?.as_array()?.clone(),
        Value::Array(items) => items.clone(),
        _ => return None,
    };
    if checks.iter().any(|item| !item.is_object()) {
        return None;
    }
    let ids = checks
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    Some(RegistryDoc { checks, ids })
}
fn expand_tilde(text: &str) -> String {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_default();
    text.replace('~', &home)
}
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
fn which(command: &str) -> bool {
    let Ok(paths) = std::env::var("PATH") else {
        return false;
    };
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&paths) {
        for ext in &extensions {
            if is_executable(&dir.join(format!("{command}{ext}"))) {
                return true;
            }
        }
        if is_executable(&dir.join(command)) {
            return true;
        }
    }
    false
}
fn wildcard_match(name: &str, pattern: &str) -> bool {
    let name: Vec<char> = name.chars().collect();
    let pattern: Vec<char> = pattern.chars().collect();
    let (mut ni, mut pi) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while ni < name.len() {
        if pi < pattern.len() && (pattern[pi] == '?' || pattern[pi] == name[ni]) {
            ni += 1;
            pi += 1;
        } else if pi < pattern.len() && pattern[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(star_pos) = star {
            pi = star_pos + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < pattern.len() && pattern[pi] == '*' {
        pi += 1;
    }
    pi == pattern.len()
}
fn has_wildcard(segment: &str) -> bool {
    segment.chars().any(|ch| matches!(ch, '*' | '?' | '['))
}
fn expand(base: &Path, segments: &[&str], found: &mut bool) {
    if *found {
        return;
    }
    if segments.is_empty() {
        *found = base.exists();
        return;
    }
    let segment = segments[0];
    if !has_wildcard(segment) {
        let next = base.join(segment);
        if segments.len() == 1 {
            if next.exists() {
                *found = true;
            }
        } else if next.is_dir() {
            expand(&next, &segments[1..], found);
        }
        return;
    }
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !wildcard_match(&name, segment) {
            continue;
        }
        let next = entry.path();
        if segments.len() == 1 {
            if next.exists() {
                *found = true;
                return;
            }
        } else if next.is_dir() {
            expand(&next, &segments[1..], found);
        }
    }
}

fn glob_exists(root: &Path, pattern: &str) -> bool {
    let expanded = expand_tilde(pattern);
    let segments: Vec<&str> = expanded.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return false;
    }
    let mut found = false;
    // Python 的 glob 对相对模式先按进程 cwd 查一次，再按 root 查一次。
    if !Path::new(&expanded).is_absolute() {
        expand(Path::new("."), &segments, &mut found);
    }
    if !found {
        expand(root, &segments, &mut found);
    }
    found
}

fn path_exists(root: &Path, pattern: &str) -> bool {
    let expanded = expand_tilde(pattern);
    let path = PathBuf::from(&expanded);
    if path.is_absolute() {
        path.exists()
    } else {
        root.join(path).exists()
    }
}
fn probe_check(
    root: &Path,
    check: &Value,
    command_cache: &mut HashMap<String, bool>,
) -> (bool, &'static str) {
    if check.get("kind").and_then(Value::as_str) == Some("cmd") {
        let run = check.get("run").and_then(Value::as_str).unwrap_or("");
        let Some(token) = run.split_whitespace().next() else {
            return (false, "no run");
        };
        if token.contains('/') || token.contains('\\') {
            let path = PathBuf::from(expand_tilde(token));
            let path = if path.is_absolute() {
                path
            } else {
                root.join(path)
            };
            return (path.exists(), "file missing");
        }
        let found = *command_cache
            .entry(token.to_string())
            .or_insert_with(|| which(token));
        return (found, "command missing");
    }
    let Some(scopes) = check.get("scope").and_then(Value::as_array) else {
        return (false, "no scope");
    };
    if scopes.is_empty() {
        return (false, "no scope");
    }
    for scope in scopes {
        let Some(pattern) = scope.as_str() else {
            continue;
        };
        let ok = if has_wildcard(pattern) {
            glob_exists(root, pattern)
        } else {
            path_exists(root, pattern)
        };
        if ok {
            return (true, "");
        }
    }
    (false, "scope files missing")
}
#[must_use]
pub fn face_registry(root: &Path, doc: Option<&RegistryDoc>) -> Face {
    let Some(doc) = doc else {
        return Face::new(
            "registry",
            Status::Unavailable,
            ".agent/checks/registry.json",
            "missing or unreadable (bad JSON?)".to_string(),
        );
    };
    let mut not_run = Vec::new();
    let mut command_cache = HashMap::new();
    for check in &doc.checks {
        let (ok, why) = probe_check(root, check, &mut command_cache);
        if !ok {
            let id = check.get("id").and_then(Value::as_str).unwrap_or("?");
            not_run.push(format!("{id}:{why}"));
        }
    }
    let status = if not_run.is_empty() {
        Status::Ok
    } else {
        Status::Warn
    };
    let mut detail = format!(
        "checks={} runnable={} not_runnable={}",
        doc.checks.len(),
        doc.checks.len() - not_run.len(),
        not_run.len()
    );
    if !not_run.is_empty() {
        detail.push_str(" [");
        detail.push_str(
            &not_run
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
        detail.push(']');
    }
    Face::new("registry", status, ".agent/checks/registry.json", detail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches_one_segment() {
        assert!(wildcard_match("a.rs", "*.rs"));
        assert!(!wildcard_match("a.txt", "*.rs"));
        assert!(wildcard_match("abc", "a?c"));
        assert!(wildcard_match("anything", "*"));
    }

    #[test]
    fn registry_accepts_bare_array_and_object() {
        let dir = std::env::temp_dir().join("trit-doctor-registry-test");
        let _ = fs::create_dir_all(dir.join(".agent/checks"));
        let text = r#"[{"id":"C001","kind":"cmd","run":"definitely-missing-cmd"}]"#;
        fs::write(dir.join(".agent/checks/registry.json"), text).unwrap();
        let doc = load(&dir).expect("bare array accepted");
        assert!(doc.ids.contains("C001"));
        let face = face_registry(&dir, Some(&doc));
        assert_eq!(face.status, Status::Warn);
        let _ = fs::remove_dir_all(&dir);
    }
}
