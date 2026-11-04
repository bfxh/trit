//! 仓库：把对象库、事件日志、认领门、提交模型装到一起。
//!
//! 目录布局（`.tritvcs/`）：
//! - `objects/`   内容寻址块与文件映射
//! - `journal.jsonl` 哈希链事件日志（追加式）
//! - `HEAD`       当前提交 id
//! - `index.json` 暂存区：树 id + 这次要提交的路径
//!
//! 与 Git 的"更强"点集中在三处：**默认拒绝未认领的写**、**每次提交带证据与标签**、
//! **所有动作进哈希链**（提交/暂存/门禁结论都能事后复核）。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::gate::{ClaimSpec, Verdict, check};
use crate::hash::Hash;
use crate::journal::{Journal, JournalError};
use crate::model::{Commit, Hlc};
use crate::store::{PutStats, Store, StoreError};

pub const DIR: &str = ".tritvcs";

#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("对象库: {0}")]
    Store(#[from] StoreError),
    #[error("事件日志: {0}")]
    Journal(#[from] JournalError),
    #[error("仓库没初始化（找不到 {0}）")]
    NotInitialized(String),
    #[error("暂存区是空的：先 `add`")]
    NothingStaged,
    #[error("未认领：{paths:?} 没有在效认领覆盖（默认拒绝；要放行加 --allow-unclaimed）")]
    Unclaimed { paths: Vec<String> },
    #[error("冲突：{others:?} 在效认领覆盖了 {paths:?}")]
    Conflict {
        others: Vec<String>,
        paths: Vec<String>,
    },
    #[error("提交 id 不存在: {0}")]
    UnknownCommit(String),
    #[error("HEAD 文件坏了: {0}")]
    BadHead(String),
}

#[derive(Clone, Debug, Default)]
pub struct CommitRequest {
    pub message: String,
    pub session: String,
    pub claims: Vec<ClaimSpec>,
    pub evidence: Vec<String>,
    pub tags: Vec<String>,
    /// 显式放行未认领的写（会记进日志，不是静默）
    pub allow_unclaimed: bool,
    pub now_secs: u64,
}

#[derive(Clone, Debug)]
pub struct CommitOutcome {
    pub id: Hash,
    pub verdict: Verdict,
    pub put: PutStats,
}

/// 暂存区内容：树 id 与本次要提交的路径（提交时按这些路径过认领门）。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Index {
    pub tree: String,
    pub paths: Vec<String>,
}

pub struct Repo {
    root: PathBuf,
    store: Store,
    journal: Journal,
}

impl Repo {
    fn dir(root: &Path) -> PathBuf {
        root.join(DIR)
    }

    pub fn init(root: impl Into<PathBuf>) -> Result<Self, RepoError> {
        let root = root.into();
        fs::create_dir_all(Self::dir(&root))?;
        let store = Store::open(Self::dir(&root))?;
        let journal = Journal::open(Self::dir(&root).join("journal.jsonl"))?;
        Ok(Self {
            root,
            store,
            journal,
        })
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self, RepoError> {
        let root = root.into();
        if !Self::dir(&root).is_dir() {
            return Err(RepoError::NotInitialized(
                Self::dir(&root).display().to_string(),
            ));
        }
        let store = Store::open(Self::dir(&root))?;
        let journal = Journal::open(Self::dir(&root).join("journal.jsonl"))?;
        Ok(Self {
            root,
            store,
            journal,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn journal(&self) -> &Journal {
        &self.journal
    }

    pub fn head(&self) -> Result<Option<Hash>, RepoError> {
        let path = Self::dir(&self.root).join("HEAD");
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        Hash::from_hex(text)
            .map(Some)
            .ok_or_else(|| RepoError::BadHead(text.to_string()))
    }

    fn write_head(&self, id: &Hash) -> Result<(), RepoError> {
        fs::write(Self::dir(&self.root).join("HEAD"), format!("{id}\n"))?;
        Ok(())
    }

    pub fn index(&self) -> Result<Option<Index>, RepoError> {
        let path = Self::dir(&self.root).join("index.json");
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(path)?;
        serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| RepoError::BadHead(format!("index.json: {e}")))
    }

    /// 暂存：把给定文件内容入库并写 index（不提交）。
    pub fn stage(
        &mut self,
        files: &BTreeMap<String, Vec<u8>>,
        now_secs: u64,
    ) -> Result<(Hash, PutStats), RepoError> {
        // 上一版的树：同路径文件的新块会拿"上一版同位置块"做增量
        let prev_files: BTreeMap<String, Hash> = match self.head()? {
            Some(head) => self
                .commit_at(&head)
                .ok()
                .and_then(|commit| self.store.tree_entries(&commit.tree).ok())
                .unwrap_or_default(),
            None => BTreeMap::new(),
        };
        let mut put = PutStats::default();
        let mut entries = BTreeMap::new();
        for (path, bytes) in files {
            let prev = prev_files.get(path).copied();
            let (id, detail) = self.store.put_file_with_prev(bytes, prev)?;
            put.new_chunks += detail.new_chunks;
            put.reused_chunks += detail.reused_chunks;
            put.bytes_written += detail.bytes_written;
            entries.insert(path.clone(), id);
        }
        let encoded =
            serde_json::to_vec(&entries).map_err(|e| RepoError::BadHead(e.to_string()))?;
        let (tree, _) = self.store.put_chunk(&encoded)?;
        let index = Index {
            tree: tree.to_hex(),
            paths: files.keys().cloned().collect(),
        };
        fs::write(
            Self::dir(&self.root).join("index.json"),
            serde_json::to_vec_pretty(&index).map_err(|e| RepoError::BadHead(e.to_string()))?,
        )?;
        self.journal.append(
            "stage",
            json!({"tree": tree.to_hex(), "paths": index.paths}),
            now_secs * 1000,
        )?;
        Ok((tree, put))
    }

    /// 提交：先过认领门，再写提交对象，最后进日志 + 推 HEAD。
    pub fn commit(&mut self, req: CommitRequest) -> Result<CommitOutcome, RepoError> {
        let index = self.index()?.ok_or(RepoError::NothingStaged)?;
        let tree =
            Hash::from_hex(&index.tree).ok_or_else(|| RepoError::BadHead(index.tree.clone()))?;
        let verdict = check(&req.claims, &req.session, &index.paths, req.now_secs);
        match &verdict {
            Verdict::Allowed => {}
            Verdict::Unclaimed if !req.allow_unclaimed => {
                return Err(RepoError::Unclaimed {
                    paths: index.paths.clone(),
                });
            }
            Verdict::Conflict { others } => {
                return Err(RepoError::Conflict {
                    others: others.clone(),
                    paths: index.paths.clone(),
                });
            }
            _ => {}
        }
        let last_ms = self.journal.events().last().map(|e| e.at_ms).unwrap_or(0);
        let hlc = Hlc::zero().tick((req.now_secs * 1000).max(last_ms));
        let mut commit = Commit::new(tree, req.session.clone(), req.message.clone(), hlc);
        if let Some(parent) = self.head()? {
            commit = commit.with_parent(parent);
        }
        for claim in &req.claims {
            commit = commit.with_claim(claim.id.clone());
        }
        for evidence in &req.evidence {
            commit = commit.with_evidence(evidence.clone());
        }
        for tag in &req.tags {
            commit = commit.with_tag(tag.clone());
        }
        let encoded = commit
            .encode()
            .map_err(|e| RepoError::BadHead(e.to_string()))?;
        let (id, _) = self.store.put_chunk(&encoded)?;
        self.journal.append(
            "commit",
            json!({
                "id": id.to_hex(),
                "tree": tree.to_hex(),
                "session": commit.session,
                "claims": commit.claims,
                "evidence": commit.evidence,
                "tags": commit.tags,
                "verdict": verdict.label(),
                "message": commit.message,
            }),
            hlc.wall_ms,
        )?;
        self.write_head(&id)?;
        Ok(CommitOutcome {
            id,
            verdict,
            put: PutStats::default(),
        })
    }

    pub fn commit_at(&self, id: &Hash) -> Result<Commit, RepoError> {
        let bytes = self.store.get_chunk(id)?;
        Commit::decode(&bytes).map_err(|e| RepoError::BadHead(e.to_string()))
    }

    /// 从 HEAD 往回走的提交链（新→旧）。
    pub fn log(&self) -> Result<Vec<(Hash, Commit)>, RepoError> {
        let mut out = Vec::new();
        let mut cursor = self.head()?;
        let mut guard = 0usize;
        while let Some(id) = cursor {
            if guard > 100_000 {
                return Err(RepoError::BadHead("提交链过长（成环？）".to_string()));
            }
            let commit = self.commit_at(&id)?;
            cursor = commit.parents.first().copied();
            out.push((id, commit));
            guard += 1;
        }
        Ok(out)
    }

    /// 全仓体检：事件链 + 对象内容 + HEAD 可达性。
    pub fn verify(&self) -> Result<VerifyReport, RepoError> {
        let events = self.journal.events().len();
        let checked = self.store.fsck().map_err(|bad| {
            RepoError::BadHead(format!(
                "对象库体检失败（{} 处）：{}",
                bad.len(),
                bad.join("；")
            ))
        })?;
        let commits = self.log()?;
        Ok(VerifyReport {
            events,
            objects: checked,
            commits: commits.len(),
            head: commits.first().map(|(id, _)| id.to_hex()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub events: usize,
    pub objects: usize,
    pub commits: usize,
    pub head: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("trit-vcs-repo-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn claim(owner: &str, paths: &[&str], expires_at: u64) -> ClaimSpec {
        ClaimSpec {
            id: format!("c-{owner}"),
            owner: owner.to_string(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            acquired_at: 1,
            expires_at,
        }
    }

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        pairs
            .iter()
            .map(|(path, body)| (path.to_string(), body.as_bytes().to_vec()))
            .collect()
    }

    #[test]
    fn stage_commit_and_log() {
        let root = temp("basic");
        let mut repo = Repo::init(&root).unwrap();
        repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
        let outcome = repo
            .commit(CommitRequest {
                message: "first".to_string(),
                session: "qoder:s1".to_string(),
                claims: vec![claim("qoder:s1", &["a.txt"], 2_000)],
                evidence: vec!["PR #1".to_string()],
                tags: vec!["demo".to_string()],
                allow_unclaimed: false,
                now_secs: 1_000,
            })
            .unwrap();
        assert_eq!(outcome.verdict, Verdict::Allowed);
        let log = repo.log().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].1.message, "first");
        assert_eq!(log[0].1.evidence, vec!["PR #1".to_string()]);
        let report = repo.verify().unwrap();
        assert_eq!(report.commits, 1);
        assert!(report.objects > 0);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unclaimed_commit_is_rejected_by_default() {
        let root = temp("unclaimed");
        let mut repo = Repo::init(&root).unwrap();
        repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
        let err = repo
            .commit(CommitRequest {
                message: "sneaky".to_string(),
                session: "qoder:s1".to_string(),
                now_secs: 1_000,
                ..Default::default()
            })
            .unwrap_err();
        assert!(matches!(err, RepoError::Unclaimed { .. }), "{err:?}");
        assert!(repo.head().unwrap().is_none(), "被拒的提交不许推进 HEAD");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unclaimed_commit_can_be_forced_and_is_recorded() {
        let root = temp("force");
        let mut repo = Repo::init(&root).unwrap();
        repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
        repo.commit(CommitRequest {
            message: "forced".to_string(),
            session: "qoder:s1".to_string(),
            allow_unclaimed: true,
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
        let last = repo.journal().events().last().unwrap();
        assert_eq!(last.kind, "commit");
        assert_eq!(last.payload["verdict"], "unclaimed", "放行也要留痕");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn conflicting_claim_blocks() {
        let root = temp("conflict");
        let mut repo = Repo::init(&root).unwrap();
        repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
        let err = repo
            .commit(CommitRequest {
                message: "clash".to_string(),
                session: "qoder:s1".to_string(),
                claims: vec![claim("zcode:zoder", &["a.txt"], 2_000)],
                now_secs: 1_000,
                ..Default::default()
            })
            .unwrap_err();
        match err {
            RepoError::Conflict { others, .. } => {
                assert_eq!(others, vec!["zcode:zoder".to_string()])
            }
            other => panic!("期望冲突，得到 {other:?}"),
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn second_commit_links_to_first() {
        let root = temp("chain");
        let mut repo = Repo::init(&root).unwrap();
        let claims = vec![claim("qoder:s1", &["a.txt"], 9_999)];
        repo.stage(&files(&[("a.txt", "one")]), 1_000).unwrap();
        let first = repo
            .commit(CommitRequest {
                message: "one".to_string(),
                session: "qoder:s1".to_string(),
                claims: claims.clone(),
                now_secs: 1_000,
                ..Default::default()
            })
            .unwrap();
        repo.stage(&files(&[("a.txt", "two")]), 2_000).unwrap();
        let second = repo
            .commit(CommitRequest {
                message: "two".to_string(),
                session: "qoder:s1".to_string(),
                claims,
                now_secs: 2_000,
                ..Default::default()
            })
            .unwrap();
        let log = repo.log().unwrap();
        assert_eq!(log.len(), 2);
        // log 是新→旧：log[0] 是第二条，它的 parent 必须是第一条
        assert_eq!(log[0].0, second.id);
        assert_eq!(log[0].1.parents, vec![first.id]);
        assert!(log[1].1.parents.is_empty(), "第一条没有父提交");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn journal_and_objects_are_verified_together() {
        let root = temp("verify");
        let mut repo = Repo::init(&root).unwrap();
        repo.stage(&files(&[("a.txt", "alpha")]), 1_000).unwrap();
        repo.commit(CommitRequest {
            message: "x".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("qoder:s1", &["a.txt"], 9_999)],
            now_secs: 1_000,
            ..Default::default()
        })
        .unwrap();
        assert!(repo.verify().is_ok());
        // 篡改 pack 里的一个字节 ⇒ verify 必须红
        let pack = Repo::dir(&root).join("objects").join("pack-000001.pack");
        let mut bytes = fs::read(&pack).unwrap();
        let at = bytes.len() / 2;
        bytes[at] ^= 0xFF;
        fs::write(&pack, bytes).unwrap();
        assert!(repo.verify().is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
