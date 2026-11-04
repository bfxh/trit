//! 工作树三件事：`status` / `checkout` / `diff`。
//!
//! 判据都用**内容**而不是 mtime：`status` 把工作树文件过一遍分块哈希，和 `HEAD` 的树比对；
//! `checkout` 默认**不覆盖未提交的改动**（除非 `--force`）——这点比 git 硬，是为了不把别人正在写的文件冲掉。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::hash::Hash;
use crate::repo::{Repo, RepoError};
use crate::store::StoreError;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Added,
    Modified,
    Deleted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Change {
    pub kind: Kind,
    pub path: String,
}

#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("仓库: {0}")]
    Repo(#[from] RepoError),
    #[error("对象库: {0}")]
    Store(#[from] StoreError),
    #[error("工作树里有 {0} 个未提交改动（会挡着 checkout）：{1:?}；确认要覆盖请加 --force")]
    Dirty(usize, Vec<String>),
}

/// 扫工作树（跳过 `.tritvcs` 与 `.git`），返回 `路径 → 内容`。
pub fn scan(root: &Path) -> Result<BTreeMap<String, Vec<u8>>, WorktreeError> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)? {
            let path = entry?.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == ".tritvcs" || name == ".git" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|_| WorktreeError::Io(std::io::Error::other("路径不在仓库下")))?
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read(&path)?);
            }
        }
    }
    Ok(out)
}

fn head_tree(repo: &Repo) -> Result<BTreeMap<String, Hash>, WorktreeError> {
    match repo.head()? {
        Some(head) => {
            let commit = repo.commit_at(&head)?;
            Ok(repo.store().tree_entries(&commit.tree)?)
        }
        None => Ok(BTreeMap::new()),
    }
}

/// 工作树 vs `HEAD`：按内容判定新增/修改/删除。
pub fn status(repo: &Repo) -> Result<Vec<Change>, WorktreeError> {
    let tree = head_tree(repo)?;
    let files = scan(repo.root())?;
    let mut changes = Vec::new();
    for (path, bytes) in &files {
        let id = repo.store().file_id_bytes(bytes)?;
        match tree.get(path) {
            None => changes.push(Change {
                kind: Kind::Added,
                path: path.clone(),
            }),
            Some(known) if *known != id => changes.push(Change {
                kind: Kind::Modified,
                path: path.clone(),
            }),
            Some(_) => {}
        }
    }
    for path in tree.keys() {
        if !files.contains_key(path) {
            changes.push(Change {
                kind: Kind::Deleted,
                path: path.clone(),
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// 把某个提交里的文件写回工作树。默认拒绝覆盖未提交改动（`--force` 才覆盖）。
pub fn checkout(repo: &Repo, commit_id: &Hash, force: bool) -> Result<usize, WorktreeError> {
    let changes = status(repo)?;
    if !force && !changes.is_empty() {
        return Err(WorktreeError::Dirty(
            changes.len(),
            changes.iter().take(5).map(|c| c.path.clone()).collect(),
        ));
    }
    let commit = repo.commit_at(commit_id)?;
    let tree = repo.store().tree_entries(&commit.tree)?;
    let mut written = 0usize;
    for (path, file_id) in &tree {
        let bytes = repo.store().get_file(file_id)?;
        let target = safe_join(repo.root(), path)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let same = fs::read(&target).map(|old| old == bytes).unwrap_or(false);
        if !same {
            fs::write(&target, &bytes)?;
            written += 1;
        }
    }
    Ok(written)
}

/// 两个提交之间的路径级差异（按内容 id 比，不做行级 diff）。
pub fn diff(repo: &Repo, a: &Hash, b: &Hash) -> Result<Vec<Change>, WorktreeError> {
    let left = repo.store().tree_entries(&repo.commit_at(a)?.tree)?;
    let right = repo.store().tree_entries(&repo.commit_at(b)?.tree)?;
    let mut out = Vec::new();
    for (path, id) in &right {
        match left.get(path) {
            None => out.push(Change {
                kind: Kind::Added,
                path: path.clone(),
            }),
            Some(old) if old != id => out.push(Change {
                kind: Kind::Modified,
                path: path.clone(),
            }),
            Some(_) => {}
        }
    }
    for path in left.keys() {
        if !right.contains_key(path) {
            out.push(Change {
                kind: Kind::Deleted,
                path: path.clone(),
            });
        }
    }
    out.sort_by(|x, y| x.path.cmp(&y.path));
    Ok(out)
}

/// 防路径穿越：树里的路径只能是仓库内的相对路径。
/// 树里的路径必须是仓库内相对路径：绝对路径、`..`、空段一律拒绝。
fn is_unsafe(rel: &str) -> bool {
    rel.starts_with('/') || rel.split('/').any(|part| part == ".." || part.is_empty())
}

fn safe_join(root: &Path, rel: &str) -> Result<PathBuf, WorktreeError> {
    let cleaned = rel.replace('\\', "/");
    if is_unsafe(&cleaned) {
        return Err(WorktreeError::Io(std::io::Error::other(format!(
            "树里的路径不合法：{rel}"
        ))));
    }
    Ok(root.join(cleaned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::ClaimSpec;
    use crate::repo::CommitRequest;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trit-vcs-wt-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// 认领要写清具体路径：trit-vcs 不接受"整仓空白声明"（声明面必须可判）。
    fn claim_for(files: &BTreeMap<String, Vec<u8>>) -> ClaimSpec {
        ClaimSpec {
            id: "c1".to_string(),
            owner: "qoder:s1".to_string(),
            paths: files.keys().cloned().collect(),
            acquired_at: 1,
            expires_at: u64::MAX / 2,
        }
    }

    fn commit_all(repo: &mut Repo, msg: &str, now: u64) -> Hash {
        let files = scan(repo.root()).unwrap();
        repo.stage(&files, now).unwrap();
        repo.commit(CommitRequest {
            message: msg.to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim_for(&files)],
            now_secs: now,
            ..Default::default()
        })
        .unwrap()
        .id
    }

    #[test]
    fn status_sees_added_modified_deleted() {
        let root = temp("status");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        fs::write(root.join("b.txt"), b"two").unwrap();
        let mut repo = Repo::init(&root).unwrap();
        let _ = commit_all(&mut repo, "base", 1_000);

        fs::write(root.join("a.txt"), b"one changed").unwrap();
        fs::remove_file(root.join("b.txt")).unwrap();
        fs::write(root.join("c.txt"), b"three").unwrap();

        let changes = status(&repo).unwrap();
        let kinds: Vec<_> = changes.iter().map(|c| (c.kind, c.path.as_str())).collect();
        assert_eq!(
            kinds,
            vec![
                (Kind::Modified, "a.txt"),
                (Kind::Deleted, "b.txt"),
                (Kind::Added, "c.txt")
            ]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn status_ignores_its_own_directory() {
        let root = temp("status-self");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        let mut repo = Repo::init(&root).unwrap();
        let _ = commit_all(&mut repo, "base", 1_000);
        assert!(
            status(&repo).unwrap().is_empty(),
            "自己的 .tritvcs 不算改动"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn checkout_restores_and_refuses_dirty_without_force() {
        let root = temp("checkout");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"v1").unwrap();
        let mut repo = Repo::init(&root).unwrap();
        let first = commit_all(&mut repo, "v1", 1_000);

        fs::write(root.join("a.txt"), b"v2").unwrap();
        let _ = commit_all(&mut repo, "v2", 2_000);

        // 有未提交改动 ⇒ 默认拒绝
        fs::write(root.join("a.txt"), b"dirty").unwrap();
        assert!(matches!(
            checkout(&repo, &first, false),
            Err(WorktreeError::Dirty(..))
        ));

        // --force 才覆盖，且内容和第一次提交一致
        let written = checkout(&repo, &first, true).unwrap();
        assert_eq!(written, 1);
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"v1");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn diff_lists_path_level_changes() {
        let root = temp("diff");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"one").unwrap();
        let mut repo = Repo::init(&root).unwrap();
        let first = commit_all(&mut repo, "v1", 1_000);

        fs::write(root.join("a.txt"), b"two").unwrap();
        fs::write(root.join("b.txt"), b"new").unwrap();
        let second = commit_all(&mut repo, "v2", 2_000);

        let changes = diff(&repo, &first, &second).unwrap();
        let kinds: Vec<_> = changes.iter().map(|c| (c.kind, c.path.as_str())).collect();
        assert_eq!(
            kinds,
            vec![(Kind::Modified, "a.txt"), (Kind::Added, "b.txt")]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn checkout_rejects_path_escape() {
        assert!(safe_join(Path::new("/repo"), "../escape").is_err());
        assert!(safe_join(Path::new("/repo"), "/abs").is_err());
        assert!(safe_join(Path::new("/repo"), "ok/inside").is_ok());
    }
}
