//! Git 双向桥 v0：trit-vcs ⇄ 真 Git 仓（用系统 `git` 做适配，不引 libgit2）。
//!
//! 导出的每个 Git 提交带 `Trit-Id/Session/Tags/Evidence` 尾注，便于反查；
//! 导入按 `git log --reverse --first-parent` 逐提交重建，文件内容逐字节搬运。
//! v0 只保证**内容与提交顺序**，不搬运 Git 的 merge 图（只走第一父链）、不搬 submodule。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::hash::Hash;
use crate::repo::{CommitRequest, Repo, RepoError};
use crate::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("仓库: {0}")]
    Repo(#[from] RepoError),
    #[error("对象库: {0}")]
    Store(#[from] StoreError),
    #[error("git {args} 失败：{stderr}")]
    Git { args: String, stderr: String },
    #[error("Git 路径不是 UTF-8：{0}")]
    BadPath(String),
    #[error("源仓还没有提交")]
    NoCommits,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportStats {
    pub commits: usize,
    pub files_written: usize,
    /// (trit 提交 id, git 提交 sha)
    pub map: Vec<(Hash, String)>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportStats {
    pub commits: usize,
    pub files: usize,
    /// (git 提交 sha, trit 提交 id)
    pub map: Vec<(String, Hash)>,
}

fn git_raw(dir: &Path, args: &[&str]) -> Result<Vec<u8>, GitError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|err| GitError::Git {
            args: args.join(" "),
            stderr: err.to_string(),
        })?;
    if !output.status.success() {
        return Err(GitError::Git {
            args: args.join(" "),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output.stdout)
}

fn git_text(dir: &Path, args: &[&str]) -> Result<String, GitError> {
    Ok(String::from_utf8_lossy(&git_raw(dir, args)?).to_string())
}

fn ensure_repo(git_dir: &Path) -> Result<(), GitError> {
    std::fs::create_dir_all(git_dir)?;
    if !git_dir.join(".git").is_dir() {
        git_raw(git_dir, &["init", "-q"])?;
    }
    // 身份只作用于本仓，避免改动用户全局配置。
    git_raw(git_dir, &["config", "user.name", "trit-vcs"])?;
    git_raw(git_dir, &["config", "user.email", "trit-vcs@localhost"])?;
    Ok(())
}

fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, GitError> {
    let cleaned = rel.replace('\\', "/");
    if cleaned.starts_with('/')
        || cleaned
            .split('/')
            .any(|part| part.is_empty() || part == "..")
    {
        return Err(GitError::BadPath(rel.to_string()));
    }
    Ok(root.join(cleaned))
}

fn walk_files(root: &Path) -> Result<Vec<(String, PathBuf)>, GitError> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|_| GitError::BadPath(path.display().to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((rel, path));
            }
        }
    }
    Ok(out)
}

fn trailer_line(id: Hash, commit: &crate::model::Commit) -> String {
    let mut out = String::new();
    out.push_str(&format!("\n\nTrit-Id: {}\n", id.to_hex()));
    out.push_str(&format!("Trit-Session: {}\n", commit.session));
    if !commit.tags.is_empty() {
        out.push_str(&format!("Trit-Tags: {}\n", commit.tags.join(",")));
    }
    for evidence in &commit.evidence {
        out.push_str(&format!("Trit-Evidence: {evidence}\n"));
    }
    out
}

/// 把 trit 仓的第一父链按旧→新写进 `git_dir`（没有 `.git` 就初始化）。
pub fn export(repo: &Repo, git_dir: &Path) -> Result<ExportStats, GitError> {
    let log = repo.log()?;
    if log.is_empty() {
        return Err(GitError::NoCommits);
    }
    ensure_repo(git_dir)?;
    let mut stats = ExportStats::default();
    for (id, commit) in log.iter().rev() {
        let entries = repo.store().tree_entries(&commit.tree)?;
        let wanted: BTreeSet<&str> = entries.keys().map(String::as_str).collect();
        // 先同步新增/修改，再删掉上一版有、这一版没有的文件。
        for (path, file_id) in &entries {
            let bytes = repo.store().get_file(file_id)?;
            let target = safe_join(git_dir, path)?;
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let same = std::fs::read(&target)
                .map(|old| old == bytes)
                .unwrap_or(false);
            if !same {
                std::fs::write(&target, &bytes)?;
                stats.files_written += 1;
            }
        }
        for (rel, path) in walk_files(git_dir)? {
            if !wanted.contains(rel.as_str()) {
                std::fs::remove_file(path)?;
            }
        }
        git_raw(git_dir, &["add", "-A"])?;
        let message = format!("{}{}", commit.message, trailer_line(*id, commit));
        git_raw(git_dir, &["commit", "-q", "--allow-empty", "-m", &message])?;
        let sha = git_text(git_dir, &["rev-parse", "HEAD"])?
            .trim()
            .to_string();
        stats.map.push((*id, sha));
        stats.commits += 1;
    }
    Ok(stats)
}

struct GitBlob {
    path: String,
    object: String,
}

fn list_blobs(git_dir: &Path, sha: &str) -> Result<Vec<GitBlob>, GitError> {
    let raw = git_raw(git_dir, &["ls-tree", "-r", "-z", sha])?;
    let mut out = Vec::new();
    for record in raw.split(|byte| *byte == 0) {
        if record.is_empty() {
            continue;
        }
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        let meta = &record[..tab];
        let path = &record[tab + 1..];
        let meta = String::from_utf8_lossy(meta);
        let mut fields = meta.split_whitespace();
        let _mode = fields.next();
        let kind = fields.next().unwrap_or("");
        let object = fields.next().unwrap_or("");
        if kind != "blob" {
            continue; // submodule / tree 不搬
        }
        let path = String::from_utf8(path.to_vec())
            .map_err(|_| GitError::BadPath(String::from_utf8_lossy(path).to_string()))?;
        out.push(GitBlob {
            path,
            object: object.to_string(),
        });
    }
    Ok(out)
}

fn strip_trailers(message: &str) -> String {
    let mut lines: Vec<&str> = message.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    while lines
        .last()
        .is_some_and(|line| line.trim_start().starts_with("Trit-"))
    {
        lines.pop();
    }
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// 把 `git_dir` 的第一父链逐提交导入当前 trit 仓（提交说明保留，证据写 git sha）。
pub fn import(repo: &mut Repo, git_dir: &Path) -> Result<ImportStats, GitError> {
    let log = git_text(
        git_dir,
        &["log", "--reverse", "--first-parent", "--format=%H"],
    )?;
    let mut stats = ImportStats::default();
    for sha in log.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut files = BTreeMap::new();
        for blob in list_blobs(git_dir, sha)? {
            files.insert(
                blob.path,
                git_raw(git_dir, &["cat-file", "blob", &blob.object])?,
            );
        }
        let message = git_text(git_dir, &["show", "-s", "--format=%B", sha])
            .map(|text| strip_trailers(&text))
            .unwrap_or_default();
        let author = git_text(git_dir, &["show", "-s", "--format=%an", sha])
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        let at = git_text(git_dir, &["show", "-s", "--format=%ct", sha])
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .unwrap_or(0);
        let now = at.max(1);
        repo.stage(&files, now)?;
        stats.files += files.len();
        let outcome = repo.commit(CommitRequest {
            message,
            session: format!("git:{}", if author.is_empty() { "bridge" } else { &author }),
            claims: Vec::new(),
            evidence: vec![format!("git:{sha}")],
            tags: vec!["git-import".to_string()],
            allow_unclaimed: true,
            witness: None,
            intent: None,
            parent_override: None,
            now_secs: now,
        })?;
        stats.map.push((sha.to_string(), outcome.id));
        stats.commits += 1;
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::gate::ClaimSpec;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trit-vcs-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn commit(repo: &mut Repo, files: &[(&str, &str)], now: u64) -> Hash {
        let map: BTreeMap<String, Vec<u8>> = files
            .iter()
            .map(|(path, body)| (path.to_string(), body.as_bytes().to_vec()))
            .collect();
        repo.stage(&map, now).unwrap();
        repo.commit(CommitRequest {
            message: format!("trit c{now}"),
            session: "qoder:git".to_string(),
            claims: vec![ClaimSpec {
                id: format!("c{now}"),
                owner: "qoder:git".to_string(),
                paths: map.keys().cloned().collect(),
                acquired_at: 1,
                expires_at: u64::MAX / 2,
            }],
            tags: vec!["vcs".to_string()],
            now_secs: now,
            ..Default::default()
        })
        .unwrap()
        .id
    }

    fn head_files(repo: &Repo) -> BTreeMap<String, Vec<u8>> {
        let head = repo.head().unwrap().unwrap();
        let tree = repo.commit_at(&head).unwrap().tree;
        let entries = repo.store().tree_entries(&tree).unwrap();
        entries
            .into_iter()
            .map(|(path, id)| (path, repo.store().get_file(&id).unwrap()))
            .collect()
    }

    #[test]
    fn trailers_are_stripped_from_imported_messages() {
        assert_eq!(
            strip_trailers("msg\n\nTrit-Id: abc\nTrit-Session: s\nTrit-Tags: vcs\n"),
            "msg"
        );
        assert_eq!(strip_trailers("plain"), "plain");
    }

    #[test]
    fn export_then_import_roundtrip_keeps_content_and_order() {
        let root = temp("roundtrip");
        let mut src = Repo::init(root.join("src")).unwrap();
        commit(&mut src, &[("a.txt", "one"), ("dir/b.txt", "two")], 1_000);
        commit(
            &mut src,
            &[("a.txt", "one changed"), ("c.txt", "three")],
            2_000,
        );
        let before = head_files(&src);

        let git_dir = root.join("git");
        let exported = export(&src, &git_dir).unwrap();
        assert_eq!(exported.commits, 2);
        assert_eq!(exported.files_written, 4);

        let log = git_text(&git_dir, &["log", "--format=%s%n%b"]).unwrap();
        assert!(
            log.contains("Trit-Tags: vcs"),
            "提交里应带 trit 尾注：{log}"
        );
        assert!(
            !git_dir.join("dir/b.txt").exists(),
            "删除必须同步到 Git 工作树"
        );

        let mut dst = Repo::init(root.join("dst")).unwrap();
        let imported = import(&mut dst, &git_dir).unwrap();
        assert_eq!(imported.commits, 2);
        assert_eq!(head_files(&dst), before, "往返后内容逐字节一致");
        let head = dst.head().unwrap().unwrap();
        assert_eq!(
            dst.commit_at(&head).unwrap().tags,
            vec!["git-import".to_string()]
        );
        let log = dst.log().unwrap();
        assert_eq!(log[0].1.message, "trit c2000", "尾注不应混进提交说明");
        assert_eq!(log[1].1.message, "trit c1000");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn import_reads_a_plain_git_repo() {
        let root = temp("import");
        let git_dir = root.join("git");
        std::fs::create_dir_all(&git_dir).unwrap();
        git_raw(&git_dir, &["init", "-q"]).unwrap();
        git_raw(&git_dir, &["config", "user.name", "bench"]).unwrap();
        git_raw(&git_dir, &["config", "user.email", "bench@example.com"]).unwrap();
        std::fs::write(git_dir.join("x.txt"), b"first").unwrap();
        git_raw(&git_dir, &["add", "-A"]).unwrap();
        git_raw(&git_dir, &["commit", "-qm", "first"]).unwrap();
        std::fs::write(git_dir.join("x.txt"), b"second").unwrap();
        git_raw(&git_dir, &["add", "-A"]).unwrap();
        git_raw(&git_dir, &["commit", "-qm", "second"]).unwrap();

        let mut repo = Repo::init(root.join("trit")).unwrap();
        let stats = import(&mut repo, &git_dir).unwrap();
        assert_eq!(stats.commits, 2);
        let files = head_files(&repo);
        assert_eq!(files.get("x.txt").unwrap().as_slice(), b"second");
        let log = repo.log().unwrap();
        assert_eq!(log[0].1.message, "second");
        assert_eq!(log[1].1.message, "first");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn export_paths_cannot_escape_the_git_worktree() {
        assert!(safe_join(Path::new("/repo"), "../escape").is_err());
        assert!(safe_join(Path::new("/repo"), "/abs").is_err());
        assert!(safe_join(Path::new("/repo"), "ok/inside").is_ok());
    }

    #[test]
    fn empty_source_is_loud() {
        let root = temp("empty");
        let repo = Repo::init(root.join("src")).unwrap();
        assert!(matches!(
            export(&repo, &root.join("git")),
            Err(GitError::NoCommits)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}
