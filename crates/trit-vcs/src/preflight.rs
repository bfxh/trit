//! 写前预检：`commit()` 的判决段抽到这里，`preflight()` 与 `commit()` 跑**同一条路**。
//!
//! 为什么必须共用：预检如果自己另写一套判断，它会和真实提交慢慢分叉——智能体信了预检、
//! 提交时还是被拒，比没有预检更糟。所以 `decide()` 是全 crate 唯一一处判
//! 「认领门 / witness / intent / 父树基线」的地方，`commit()` 只负责它之后的写盘。
//!
//! 预检只读：不写对象、不写 index、不进哈希链、不动 HEAD（测试按这四样逐条钉住）。

use std::collections::HashSet;

use serde::Serialize;

use crate::gate::{Verdict, check};
use crate::hash::Hash;
use crate::model::Intent;
use crate::repo::{CommitRequest, Policy, Repo, RepoError, collapsed_paths, require_passing};
use crate::store::Store;
use crate::witness::{self, Witness};

/// `commit()` 判决段的产物（内部形状，带真实类型，供 `commit()` 继续写盘）。
#[derive(Clone, Debug)]
pub struct Decision {
    pub tree: Hash,
    pub verdict: Verdict,
    /// 折叠后的判决面（认领门实际比较的东西）。
    pub touched: Vec<String>,
    /// 折叠前的暂存条目数（大提交折叠后才看得见规模）。
    pub staged: usize,
    pub claim_ids: Vec<String>,
    /// `(摘要, 解析后的见证)`；摘要 = 见证字节的 BLAKE3，与写盘后 `put_chunk` 的 id 同一个值。
    pub witness: Option<(Hash, Witness)>,
    pub parent: Option<Hash>,
    pub base_tree: Option<Hash>,
    pub intent: Option<Intent>,
    pub policy: Policy,
}

/// 预检的可序列化投影（`Hash` 的 serde 是 32 字节数组，这里一律给十六进制字符串）。
#[derive(Clone, Debug, Serialize)]
pub struct Facts {
    pub verdict: &'static str,
    pub tree: String,
    pub parent: Option<String>,
    pub base_tree: Option<String>,
    pub touched: Vec<String>,
    pub staged_paths: usize,
    pub claims: Vec<String>,
    pub witness: Option<String>,
    pub witness_exit: Option<i32>,
    pub intent: Option<IntentFacts>,
    pub require_witness: bool,
    pub require_intent: bool,
    pub size: SizeFacts,
}

#[derive(Clone, Debug, Serialize)]
pub struct IntentFacts {
    pub goal: String,
    pub accept: Vec<String>,
    pub base: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct SizeFacts {
    pub files: usize,
    pub objects: usize,
    pub bytes: u64,
}

/// 唯一一处判决。`commit()` 与 `preflight()` 都走它。
pub(crate) fn decide(repo: &Repo, req: &CommitRequest) -> Result<Decision, RepoError> {
    let index = repo.index()?.ok_or(RepoError::NothingStaged)?;
    let tree = Hash::from_hex(&index.tree).ok_or_else(|| RepoError::BadHead(index.tree.clone()))?;
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
    let policy = repo.policy()?;
    let witness: Option<(Hash, Witness)> = match req.witness.as_deref() {
        Some(bytes) => {
            let parsed = witness::verify(bytes, tree)?;
            require_passing(&parsed)?;
            Some((Hash::of(bytes), parsed))
        }
        None if policy.require_witness => return Err(RepoError::WitnessRequired),
        None => None,
    };
    let parent = req.parent_override.or(repo.head()?);
    let base_tree = match parent {
        Some(id) => Some(repo.commit_at(&id)?.tree),
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
    Ok(Decision {
        tree,
        verdict,
        touched: gate_paths,
        staged: index.paths.len(),
        claim_ids: req.claims.iter().map(|claim| claim.id.clone()).collect(),
        witness,
        parent,
        base_tree,
        intent,
        policy,
    })
}

/// 这次提交会落多少对象、多少字节（**只读**：逐个数文件映射与去重后的块）。
///
/// 成本随暂存文件数线性，预检每次都算：它答的是"这次提交多大"，不是"仓多大"。
pub(crate) fn size_of(store: &Store, tree: &Hash) -> Result<SizeFacts, RepoError> {
    let entries = store.tree_entries(tree)?;
    let mut chunks = HashSet::new();
    let mut bytes = 0u64;
    for id in entries.values() {
        let map = store.file_map(id)?;
        bytes += map.size;
        chunks.extend(map.chunks.iter().copied());
    }
    Ok(SizeFacts {
        files: entries.len(),
        // 树对象 + 每个文件映射 + 去重后的数据块
        objects: 1 + entries.len() + chunks.len(),
        bytes,
    })
}

impl Repo {
    /// 跑一遍提交判决，但什么都不写。返回的 `Facts` 与真提交用的是同一处判决。
    pub fn preflight(&self, req: &CommitRequest) -> Result<Facts, RepoError> {
        let decision = decide(self, req)?;
        let size = size_of(self.store(), &decision.tree)?;
        Ok(project(&decision, size))
    }
}

fn project(d: &Decision, size: SizeFacts) -> Facts {
    Facts {
        verdict: d.verdict.label(),
        tree: d.tree.to_hex(),
        parent: d.parent.map(|id| id.to_hex()),
        base_tree: d.base_tree.map(|id| id.to_hex()),
        touched: d.touched.clone(),
        staged_paths: d.staged,
        claims: d.claim_ids.clone(),
        witness: d.witness.as_ref().map(|(digest, _)| digest.to_hex()),
        witness_exit: d.witness.as_ref().map(|(_, parsed)| parsed.predicate.exit),
        intent: d.intent.as_ref().map(|intent| IntentFacts {
            goal: intent.goal.clone(),
            accept: intent.accept.clone(),
            base: intent.base.map(|base| base.to_hex()),
        }),
        require_witness: d.policy.require_witness,
        require_intent: d.policy.require_intent,
        size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gate::ClaimSpec;
    use crate::repo::CommitRequest;
    use std::collections::BTreeMap;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trit-vcs-preflight-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        pairs
            .iter()
            .map(|(path, body)| (path.to_string(), body.to_string().into_bytes()))
            .collect()
    }

    fn claim(id: &str, owner: &str, paths: &[&str], expires: u64) -> ClaimSpec {
        ClaimSpec {
            id: id.to_string(),
            owner: owner.to_string(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
            acquired_at: 1,
            expires_at: expires,
        }
    }

    fn request(paths: &[&str], expires: u64) -> CommitRequest {
        CommitRequest {
            message: "m".to_string(),
            session: "qoder:s1".to_string(),
            claims: vec![claim("mine", "qoder:s1", paths, expires)],
            now_secs: 1_000,
            ..Default::default()
        }
    }

    fn repo_with(dir: &std::path::Path, corpus: &BTreeMap<String, Vec<u8>>) -> Repo {
        std::fs::create_dir_all(dir).unwrap();
        let mut repo = Repo::init(dir).unwrap();
        repo.stage(corpus, 1_000).unwrap();
        repo
    }

    /// 核心不变量：预检和真实提交必须给同一个判决——分叉了就是预检在骗智能体。
    #[test]
    fn preflight_and_commit_share_one_verdict() {
        let root = temp("same-verdict");
        let corpus = files(&[("a.txt", "a\n"), ("b.txt", "b\n")]);

        // 1) 放行：预检说 allowed，提交也成功，且指的是同一棵树
        let mut repo = repo_with(&root.join("r"), &corpus);
        let facts = repo
            .preflight(&request(&["a.txt", "b.txt"], 9_000))
            .unwrap();
        assert_eq!(facts.verdict, "allowed");
        let outcome = repo
            .commit(request(&["a.txt", "b.txt"], 9_000))
            .expect("同一请求提交应成功");
        assert_eq!(
            facts.tree,
            repo.commit_at(&outcome.id).unwrap().tree.to_hex(),
            "预检报的树必须就是真提交落的树"
        );

        // 2) 没认领：两边都是 unclaimed
        let mut repo2 = repo_with(&root.join("r2"), &corpus);
        let empty = CommitRequest {
            claims: vec![],
            ..request(&[], 9_000)
        };
        let pre = repo2.preflight(&empty).unwrap_err();
        let real = repo2.commit(empty).unwrap_err();
        assert!(matches!(pre, RepoError::Unclaimed { .. }), "{pre}");
        assert_eq!(format!("{pre}"), format!("{real}"));

        // 3) 撞车：两边点名同一个对手
        let mut repo3 = repo_with(&root.join("r3"), &corpus);
        let clash = CommitRequest {
            claims: vec![
                claim("mine", "qoder:s1", &["a.txt"], 9_000),
                claim("theirs", "zcode:other", &["a.txt"], 9_000),
            ],
            ..request(&[], 1_000)
        };
        let pre = repo3.preflight(&clash).unwrap_err();
        let real = repo3.commit(clash).unwrap_err();
        assert_eq!(crate::remediation::kind(&pre), "claim_conflict");
        assert_eq!(format!("{pre}"), format!("{real}"));
        assert!(
            format!("{pre}").contains("zcode:other"),
            "判决必须点名对手：{pre}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preflight_writes_nothing() {
        let root = temp("read-only");
        let corpus = files(&[("a.txt", "a\n"), ("dir/b.txt", "b\n"), ("c.txt", "c\n")]);
        let mut repo = repo_with(&root.join("r"), &corpus);
        let req = request(&["a.txt", "dir/b.txt", "c.txt"], 9_000);

        let objects = repo.store().stats().unwrap().objects;
        let events = repo.journal().events().len();
        let facts = repo.preflight(&req).unwrap();

        assert_eq!(
            repo.store().stats().unwrap().objects,
            objects,
            "预检不许写对象"
        );
        assert_eq!(repo.journal().events().len(), events, "预检不进哈希链");
        assert!(repo.head().unwrap().is_none(), "预检不动 HEAD");
        assert_eq!(facts.size.files, 3);
        assert!(facts.size.bytes > 0);

        // 真提交才会推 HEAD、才会进链
        let committed = repo.commit(req).unwrap();
        assert_eq!(repo.head().unwrap(), Some(committed.id));
        assert!(repo.journal().events().len() > events);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preflight_reports_collapsed_scope_and_size() {
        let root = temp("scope");
        let mut corpus = BTreeMap::new();
        for i in 0..12 {
            corpus.insert(
                format!("gen/d{i:02}/f.txt"),
                format!("body {i}\n").into_bytes(),
            );
        }
        let repo = repo_with(&root.join("r"), &corpus);
        let wide = CommitRequest {
            claims: vec![claim("scope", "qoder:s1", &["gen"], 9_000)],
            ..request(&[], 1_000)
        };
        let facts = repo.preflight(&wide).unwrap();
        assert_eq!(facts.staged_paths, 12);
        assert_eq!(facts.touched, vec!["gen".to_string()], "整片范围应折成一条");
        assert_eq!(facts.size.files, 12);
        assert!(facts.size.bytes > 0, "规模读数必须带字节数");
        assert_eq!(facts.claims, vec!["scope".to_string()]);
        assert_eq!(facts.witness, None);
        assert_eq!(facts.witness_exit, None);
        assert!(!facts.require_witness);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preflight_surfaces_intent_base_binding() {
        let root = temp("intent");
        let corpus = files(&[("a.txt", "a\n")]);
        let mut repo = repo_with(&root.join("r"), &corpus);
        let id = repo.commit(request(&["a.txt"], 9_000)).unwrap().id;
        let parent_tree = repo.commit_at(&id).unwrap().tree;

        repo.stage(&files(&[("a.txt", "a\nb\n")]), 1_001).unwrap();
        let good = CommitRequest {
            intent: Some(Intent {
                goal: "加一行".to_string(),
                accept: vec!["cargo test -p trit-vcs".to_string()],
                base: Some(parent_tree),
            }),
            ..request(&["a.txt"], 9_000)
        };
        let facts = repo.preflight(&good).unwrap();
        let intent = facts.intent.expect("预检要带出 intent");
        assert_eq!(intent.base.unwrap(), parent_tree.to_hex());

        let bad = CommitRequest {
            intent: Some(Intent {
                goal: "加一行".to_string(),
                accept: vec!["x".to_string()],
                base: Some(Hash::of(b"not-the-parent-tree")),
            }),
            ..request(&["a.txt"], 9_000)
        };
        assert!(
            matches!(
                repo.preflight(&bad).unwrap_err(),
                RepoError::IntentBaseMismatch { .. }
            ),
            "预检必须把基线错配也拦下来"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
