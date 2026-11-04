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
use crate::model::{Commit, Hlc, Intent};
use crate::store::{PutStats, Store, StoreError};
use crate::witness::{self, Witness, WitnessError};

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
    #[error("仓库策略要求每次提交带 witness；先跑 `trit-vcs witness -- <命令>`")]
    WitnessRequired,
    #[error("witness 不成立：{0}")]
    Witness(#[from] WitnessError),
    #[error("witness 记录的命令退出码是 {0}；失败的运行不能当提交证据")]
    WitnessFailed(i32),
    #[error("提交 {0} 没有 witness：不能作为合并候选")]
    Unwitnessed(String),
    #[error("仓库策略要求提交带 intent（goal + 至少一条 accept）")]
    IntentRequired,
    #[error("intent 不成立：{0}")]
    IntentInvalid(String),
    #[error("intent 基于的树 {declared} 与父提交的树 {actual} 不一致")]
    IntentBaseMismatch { declared: String, actual: String },
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
    /// 绑定本次树的测试见证（canonical JSON 字节）；策略要求时不可为空
    pub witness: Option<Vec<u8>>,
    /// 改动意图；`base` 为空时由提交自动填成父提交的树
    pub intent: Option<Intent>,
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

/// 仓库策略（`.tritvcs/policy.json`）。默认全关：不影响别项目，成熟后逐项目 opt-in。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Policy {
    #[serde(default)]
    pub require_witness: bool,
    #[serde(default)]
    pub require_intent: bool,
}

/// 认领范围已经覆盖的路径折叠成范围本身。
/// 10k 文件的提交不必逐路径过门（门的路径上限是 256），判定等价：范围覆盖 ⇒ 范围与路径同权。
fn collapsed_paths(paths: &[String], claims: &[ClaimSpec]) -> Vec<String> {
    fn covers(scope: &str, path: &str) -> bool {
        scope == path
            || (path.len() > scope.len()
                && path.starts_with(scope)
                && path.as_bytes().get(scope.len()) == Some(&b'/'))
    }
    let mut out: Vec<String> = Vec::new();
    for path in paths {
        if claims
            .iter()
            .any(|claim| claim.paths.iter().any(|scope| covers(scope, path)))
        {
            continue;
        }
        out.push(path.clone());
    }
    for claim in claims {
        for scope in &claim.paths {
            if paths.iter().any(|path| covers(scope, path)) {
                out.push(scope.clone());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 见证必须是"跑通"的那种；失败的运行单独报，避免和格式错误混在一起。
fn require_passing(witness: &Witness) -> Result<(), RepoError> {
    match witness.check_passed() {
        Ok(()) => Ok(()),
        Err(WitnessError::Failed(code)) => Err(RepoError::WitnessFailed(code)),
        Err(other) => Err(RepoError::Witness(other)),
    }
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

    /// 仓库策略；文件坏了就让调用方失败（不能静默退回"不要求见证"）。
    pub fn policy(&self) -> Result<Policy, RepoError> {
        let path = Self::dir(&self.root).join("policy.json");
        if !path.is_file() {
            return Ok(Policy::default());
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(|e| RepoError::BadHead(format!("policy.json: {e}")))
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
        let gate_paths = collapsed_paths(&index.paths, &req.claims);
        let verdict = check(&req.claims, &req.session, &gate_paths, req.now_secs);
        match &verdict {
            Verdict::Allowed => {}
            Verdict::Unclaimed if !req.allow_unclaimed => {
                return Err(RepoError::Unclaimed {
                    paths: gate_paths.clone(),
                });
            }
            Verdict::Conflict { others } => {
                return Err(RepoError::Conflict {
                    others: others.clone(),
                    paths: gate_paths.clone(),
                });
            }
            _ => {}
        }
        let policy = self.policy()?;
        let witness: Option<(Hash, Witness)> = match req.witness.as_deref() {
            Some(bytes) => {
                let parsed = witness::verify(bytes, tree)?;
                require_passing(&parsed)?;
                let (digest, _) = self.store.put_chunk(bytes)?;
                Some((digest, parsed))
            }
            None if policy.require_witness => return Err(RepoError::WitnessRequired),
            None => None,
        };
        let parent = self.head()?;
        let base_tree = match parent {
            Some(id) => Some(self.commit_at(&id)?.tree),
            None => None,
        };
        let intent = match req.intent.clone() {
            Some(mut intent) => {
                if intent.goal.trim().is_empty() {
                    return Err(RepoError::IntentInvalid("goal 不能为空".to_string()));
                }
                if intent.accept.iter().all(|item| item.trim().is_empty()) {
                    return Err(RepoError::IntentInvalid(
                        "至少写一条 accept 判据".to_string(),
                    ));
                }
                match (intent.base, base_tree) {
                    (Some(declared), Some(actual)) if declared != actual => {
                        return Err(RepoError::IntentBaseMismatch {
                            declared: declared.to_hex(),
                            actual: actual.to_hex(),
                        });
                    }
                    (Some(_), None) => {
                        return Err(RepoError::IntentInvalid(
                            "第一条提交没有父树，intent.base 必须留空".to_string(),
                        ));
                    }
                    (None, Some(actual)) => intent.base = Some(actual),
                    _ => {}
                }
                Some(intent)
            }
            None if policy.require_intent => return Err(RepoError::IntentRequired),
            None => None,
        };
        let last_ms = self.journal.events().last().map(|e| e.at_ms).unwrap_or(0);
        let hlc = Hlc::zero().tick((req.now_secs * 1000).max(last_ms));
        let mut commit = Commit::new(tree, req.session.clone(), req.message.clone(), hlc);
        if let Some((digest, _)) = &witness {
            commit = commit.with_witness(*digest);
        }
        if let Some(intent) = &intent {
            commit = commit.with_intent(intent.clone());
        }
        if let Some(parent) = parent {
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
                "witness": witness.as_ref().map(|(digest, _)| digest.to_hex()),
                "intent": intent.as_ref().map(|intent| json!({
                    "goal": intent.goal,
                    "accept": intent.accept.len(),
                    "base": intent.base.map(|base| base.to_hex()),
                })),
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

    /// 合并准入：见证文件必须与提交里记录的摘要一致、绑定该提交的树、且运行通过。
    /// 这是"通过就合、不通过就通知智能体"那条链里唯一可信的判据。
    pub fn admit(&self, id: &Hash, witness_bytes: &[u8]) -> Result<Witness, RepoError> {
        let commit = self.commit_at(id)?;
        let recorded = commit
            .witness
            .ok_or_else(|| RepoError::Unwitnessed(id.short().to_string()))?;
        witness::check_file(recorded, witness_bytes)?;
        let parsed = witness::verify(witness_bytes, commit.tree)?;
        require_passing(&parsed)?;
        Ok(parsed)
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
        let mut witnessed = 0usize;
        for (id, commit) in &commits {
            let Some(digest) = commit.witness else {
                continue;
            };
            witnessed += 1;
            let bytes = self.store.get_chunk(&digest)?;
            let parsed = witness::verify(&bytes, commit.tree).map_err(|e| {
                RepoError::BadHead(format!("提交 {} 的 witness 不成立：{e}", id.short()))
            })?;
            parsed.check_passed().map_err(|e| {
                RepoError::BadHead(format!("提交 {} 的 witness 不可用：{e}", id.short()))
            })?;
        }
        Ok(VerifyReport {
            events,
            objects: checked,
            commits: commits.len(),
            witnessed,
            head: commits.first().map(|(id, _)| id.to_hex()),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyReport {
    pub events: usize,
    pub objects: usize,
    pub commits: usize,
    pub witnessed: usize,
    pub head: Option<String>,
}

#[cfg(test)]
mod tests;
