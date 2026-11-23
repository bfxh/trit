//! git 面：冷跑 git 子进程，暖跑按“树指纹 + git 状态”缓存；输出与 Python 面一致。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::cache::{CachedFace, read_json, write_json};
use crate::gates;
use crate::{Face, Status};

const GIT_CACHE_REL: &str = ".agent/trit-doctor-git-cache.json";

#[derive(Debug, Serialize, Deserialize)]
struct GitCache {
    key: String,
    face: CachedFace,
}

fn mtime_size(path: &Path) -> String {
    match fs::metadata(path) {
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(-1, |duration| duration.as_nanos() as i64);
            format!("{mtime}:{}", meta.len())
        }
        Err(_) => "gone".to_string(),
    }
}

fn git_dir(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    if dot_git.is_file() {
        let text = fs::read_to_string(&dot_git).ok()?;
        let path = text.trim().strip_prefix("gitdir:")?.trim();
        let path = PathBuf::from(path);
        return Some(if path.is_absolute() {
            path
        } else {
            root.join(path)
        });
    }
    None
}

fn refs_fingerprint(git_dir: &Path) -> String {
    fn walk(base: &Path, dir: &Path, buffer: &mut String) {
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
                dirs.push(entry);
            } else {
                files.push(entry);
            }
        }
        files.sort_by_key(fs::DirEntry::file_name);
        dirs.sort_by_key(fs::DirEntry::file_name);
        for entry in files {
            let path = entry.path();
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            buffer.push_str(&format!("{relative}|{}\n", mtime_size(&path)));
        }
        for entry in dirs {
            walk(base, &entry.path(), buffer);
        }
    }

    let mut buffer = String::new();
    let refs = git_dir.join("refs");
    walk(&refs, &refs, &mut buffer);
    buffer.push_str(&format!(
        "packed-refs|{}",
        mtime_size(&git_dir.join("packed-refs"))
    ));
    buffer
}

fn cache_key(root: &Path, fingerprint: &str) -> String {
    let Some(dir) = git_dir(root) else {
        return format!("{fingerprint}|no-git-dir");
    };
    let head = fs::read_to_string(dir.join("HEAD")).unwrap_or_default();
    format!(
        "{}|{}|{}|{}",
        fingerprint,
        head.trim(),
        mtime_size(&dir.join("index")),
        refs_fingerprint(&dir),
    )
}

fn io_name(error: &std::io::Error) -> &'static str {
    match error.kind() {
        std::io::ErrorKind::NotFound => "FileNotFoundError",
        std::io::ErrorKind::PermissionDenied => "PermissionError",
        std::io::ErrorKind::TimedOut => "TimeoutExpired",
        _ => "OSError",
    }
}

fn run_git(root: &Path, args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new("git").args(args).current_dir(root).output()
}

#[must_use]
pub fn face_git(root: &Path, fingerprint: &str, refresh: bool) -> Face {
    let key = cache_key(root, fingerprint);
    if !refresh
        && let Some(cache) = read_json::<GitCache>(&root.join(GIT_CACHE_REL))
        && cache.key == key
    {
        let status = Status::from_code(&cache.face.status).unwrap_or(Status::Unavailable);
        return Face::new("git", status, &cache.face.evidence, cache.face.detail);
    }

    let check = match run_git(root, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(output) => output,
        Err(error) => {
            return Face::new(
                "git",
                Status::Unavailable,
                "git",
                format!("git unavailable ({})", io_name(&error)),
            );
        }
    };
    if !check.status.success() || String::from_utf8_lossy(&check.stdout).trim() != "true" {
        return Face::new(
            "git",
            Status::Unavailable,
            "git",
            "not a git work tree".to_string(),
        );
    }

    let status = match run_git(root, &["status", "--porcelain"]) {
        Ok(output) => output,
        Err(error) => {
            return Face::new(
                "git",
                Status::Unavailable,
                "git",
                format!("git command failed ({})", io_name(&error)),
            );
        }
    };
    let for_each_ref = match run_git(
        root,
        &["for-each-ref", "--format=%(objectname)", "refs/remotes"],
    ) {
        Ok(output) => output,
        Err(error) => {
            return Face::new(
                "git",
                Status::Unavailable,
                "git",
                format!("git command failed ({})", io_name(&error)),
            );
        }
    };
    let branch_output = match run_git(
        root,
        &[
            "for-each-ref",
            "--format=%(objectname)|%(refname:short)",
            "refs/heads",
        ],
    ) {
        Ok(output) => output,
        Err(error) => {
            return Face::new(
                "git",
                Status::Unavailable,
                "git",
                format!("git command failed ({})", io_name(&error)),
            );
        }
    };

    let dirty = String::from_utf8_lossy(&status.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    let remote_refs: std::collections::HashSet<String> =
        String::from_utf8_lossy(&for_each_ref.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect();
    let mut branches = 0usize;
    let mut unpushed = Vec::new();
    for line in String::from_utf8_lossy(&branch_output.stdout).lines() {
        let Some((sha, name)) = line.split_once('|') else {
            continue;
        };
        branches += 1;
        if !remote_refs.contains(sha) {
            unpushed.push(name.to_string());
        }
    }

    let evidence = format!(
        "status exit={}; for-each-ref exit={}",
        status.status.code().unwrap_or(-1),
        for_each_ref.status.code().unwrap_or(-1)
    );
    let mut detail = format!(
        "dirty={dirty} branches={branches} unpushed={}",
        unpushed.len()
    );
    if !unpushed.is_empty() {
        detail.push_str(" first=");
        detail.push_str(
            &unpushed
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(","),
        );
    }
    let face_status = if dirty > 0 || !unpushed.is_empty() {
        Status::Warn
    } else {
        Status::Ok
    };
    let face = Face::new("git", face_status, &evidence, detail);
    let cache = GitCache {
        key,
        face: CachedFace {
            status: face.status.as_str().to_string(),
            evidence: face.evidence.clone(),
            detail: face.detail.clone(),
        },
    };
    if gates::cache_allowed(root) {
        write_json(&root.join(GIT_CACHE_REL), &cache);
    }
    face
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_repo_is_unavailable() {
        let root = std::env::temp_dir().join("trit-doctor-git-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let face = face_git(&root, "0000000000000000", true);
        assert_eq!(face.status, Status::Unavailable);
        let _ = fs::remove_dir_all(&root);
    }
}
