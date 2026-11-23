//! 门面：复用 Python doctor 的 `.agent/doctor-cache.json`，指纹算法逐字节对齐。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use trit_core::fingerprint::sha256_hex;

use crate::cache::{CachedFace, read_json, write_json};
use crate::{Face, Status};

pub const CACHE_REL: &str = ".agent/doctor-cache.json";
pub const SCAN_SKIP: [&str; 18] = [
    ".git",
    ".journal",
    ".agent",
    "target",
    "node_modules",
    ".trash",
    ".qoder",
    "__pycache__",
    ".venv",
    "dist",
    "build",
    "obj",
    ".pytest_cache",
    ".ruff_cache",
    ".mypy_cache",
    ".turbo",
    ".next",
    "coverage",
];
const GATE_SCRIPTS: [(&str, &str); 2] = [
    ("privacy", "scripts/privacy_scan.py"),
    ("docs", "scripts/docs_hygiene.py"),
];

#[derive(Debug, Serialize, Deserialize, Default)]
struct GateCache {
    fp: String,
    faces: BTreeMap<String, CachedFace>,
}

fn mtime_ns(path: &Path) -> Option<(i64, u64)> {
    let meta = fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some((modified.as_nanos() as i64, meta.len()))
}

fn walk(root: &Path, dir: &Path, buffer: &mut String) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<fs::DirEntry> = Vec::new();
    let mut dirs: Vec<fs::DirEntry> = Vec::new();
    for entry in read_dir.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !SCAN_SKIP.contains(&name.as_str()) {
                dirs.push(entry);
            }
        } else {
            files.push(entry);
        }
    }
    // 与 Python os.walk 同序：当前目录的文件先全部落哈希，再按目录名递归。
    files.sort_by_key(fs::DirEntry::file_name);
    dirs.sort_by_key(fs::DirEntry::file_name);
    for entry in files {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        match mtime_ns(&path) {
            Some((mtime, size)) => {
                buffer.push_str(&format!("{relative}|{mtime}|{size}\n"));
            }
            None => {
                buffer.push_str(&format!("{relative}|gone\n"));
            }
        }
    }
    for entry in dirs {
        walk(root, &entry.path(), buffer);
    }
}

/// 与 `tools/doctor.py::tree_fingerprint` 同算法：排序后的 `(相对路径, mtime_ns, size)`，
/// SHA-256 取前 16 位十六进制。
#[must_use]
pub fn tree_fingerprint(root: &Path) -> String {
    let mut buffer = String::with_capacity(1 << 20);
    walk(root, root, &mut buffer);
    let hash = sha256_hex(buffer.as_bytes());
    hash[..16].to_string()
}

fn python_exe() -> String {
    std::env::var("PYTHON").unwrap_or_else(|_| {
        if cfg!(windows) {
            "python".to_string()
        } else {
            "python3".to_string()
        }
    })
}

pub(crate) fn cache_allowed(root: &Path) -> bool {
    let Ok(check) = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .output()
    else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&check.stdout);
    if !check.status.success() || stdout.trim() != "true" {
        return true;
    }
    Command::new("git")
        .args(["check-ignore", "-q", CACHE_REL])
        .current_dir(root)
        .output()
        .is_ok_and(|output| output.status.success())
}

#[must_use]
pub fn face_gates(root: &Path, fingerprint: &str, refresh: bool) -> Face {
    if !refresh
        && let Some(cache) = read_json::<GateCache>(&root.join(CACHE_REL))
        && cache.fp == fingerprint
        && let Some(gates) = cache.faces.get("gates")
    {
        let status = Status::from_code(&gates.status).unwrap_or(Status::Unavailable);
        let detail = format!(
            "{} [cached fp={} --refresh to re-verify]",
            gates.detail,
            &fingerprint[..8.min(fingerprint.len())]
        );
        return Face::new("gates", status, &gates.evidence, detail);
    }

    let mut results: Vec<(&str, i32)> = Vec::new();
    let mut missing: Vec<&str> = Vec::new();
    for (name, relative) in GATE_SCRIPTS {
        let script = root.join(relative);
        if !script.is_file() {
            missing.push(name);
            continue;
        }
        let code = Command::new(python_exe())
            .arg(&script)
            .current_dir(root)
            .output()
            .map_or(-1, |output| output.status.code().unwrap_or(-1));
        results.push((name, code));
    }
    if results.is_empty() {
        return Face::new(
            "gates",
            Status::Unavailable,
            "scripts/privacy_scan.py, scripts/docs_hygiene.py",
            "no gate script present at this root".to_string(),
        );
    }
    let evidence = results
        .iter()
        .map(|(name, code)| format!("{name} exit={code}"))
        .collect::<Vec<_>>()
        .join("; ");
    let mut detail = evidence.clone();
    if !missing.is_empty() {
        detail.push_str(&format!("; missing: {}", missing.join(",")));
    }
    let failed: Vec<&str> = results
        .iter()
        .filter(|(_, code)| *code != 0)
        .map(|(name, _)| *name)
        .collect();
    let status = if failed.is_empty() {
        Status::Ok
    } else {
        let script = if failed[0] == "privacy" {
            "privacy_scan.py"
        } else {
            "docs_hygiene.py"
        };
        detail.push_str(&format!("; rerun: python scripts/{script}"));
        Status::Fail
    };
    let face = Face::new("gates", status, &evidence, detail.clone());
    if cache_allowed(root) {
        let mut cache = GateCache {
            fp: fingerprint.to_string(),
            faces: BTreeMap::new(),
        };
        cache.faces.insert(
            "gates".to_string(),
            CachedFace {
                status: face.status.as_str().to_string(),
                evidence: face.evidence.clone(),
                detail,
            },
        );
        write_json(&root.join(CACHE_REL), &cache);
    }
    face
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tree_fingerprint_is_stable() {
        let root = std::env::temp_dir().join("trit-doctor-fp-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let fp = tree_fingerprint(&root);
        assert_eq!(fp, "e3b0c44298fc1c14");
        let _ = fs::remove_dir_all(&root);
    }
}
