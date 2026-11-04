//! 中央仓 + 按需物化：`publish` 把命名 ref **往前推**（非后代判红），`pull` 只取一个目录的对象并稀疏物化。
//!
//! 搬运走内容寻址：目标已有的块直接跳过，所以第二次拉几乎零字节。
//! v0 是本地/离线（中央仓是另一个目录）；网络、鉴权、跨机认领表都还没做，不在这里宣称。

use crate::bundle::{self, TransferStats};
use crate::hash::Hash;
use crate::repo::{Repo, RepoError};
use crate::worktree::{self, WorktreeError};

#[derive(Debug, thiserror::Error)]
pub enum CentralError {
    #[error("仓库: {0}")]
    Repo(#[from] RepoError),
    #[error("传输: {0}")]
    Bundle(#[from] bundle::BundleError),
    #[error("工作树: {0}")]
    Worktree(#[from] WorktreeError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishStats {
    pub commit: Hash,
    pub transfer: TransferStats,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullStats {
    pub commit: Hash,
    pub transfer: TransferStats,
    pub written: usize,
    pub skipped: usize,
}

/// 补父链的上限：再多也不该一次搬完整个仓库历史。
const ANCESTOR_CAP: usize = 4_096;

/// 把客户端的某个提交（缺省 HEAD）搬到中央仓，并写上命名 ref。
///
/// ref **只允许往前推**：新提交必须是现有 ref 的后代，否则判红（`--force` 才改写）。
/// 多个智能体共用一个中央仓时，甩掉别人的历史比丢一个文件贵得多。
/// 顺序是先搬对象再判 ref：对象内容寻址且不可变，搬了不挪 ref 盖不掉任何东西；
/// 判红时 ref 一定还在原位（非法名字在搬任何东西之前就被拒）。
pub fn publish(
    client: &Repo,
    central: &mut Repo,
    name: &str,
    commit: Option<&Hash>,
    force: bool,
) -> Result<PublishStats, CentralError> {
    Repo::validate_ref(name)?;
    let resolved = match commit {
        Some(id) => *id,
        None => client.head()?.ok_or(RepoError::NothingStaged)?,
    };
    let mut objects = bundle::commit_closure(client.store(), resolved)?;
    objects.extend(bundle::missing_ancestors(
        client.store(),
        central.store(),
        resolved,
        ANCESTOR_CAP,
    )?);
    let transfer = bundle::transfer(client.store(), central.store(), &objects)?;
    if let Some(current) = central.ref_commit(name)? {
        let descendant =
            current == resolved || central.merge_base(&resolved, &current)? == Some(current);
        if !descendant && !force {
            return Err(CentralError::Repo(RepoError::NonFastForward {
                name: name.to_string(),
                current: current.short(),
                incoming: resolved.short(),
            }));
        }
    }
    central.set_ref(name, &resolved)?;
    Ok(PublishStats {
        commit: resolved,
        transfer,
    })
}

/// 从中央仓拉一个 ref：`prefix` 给定时只搬该目录的树清单与对象闭包，并只物化该目录。
pub fn pull(
    central: &Repo,
    client: &mut Repo,
    name: &str,
    prefix: Option<&str>,
    force: bool,
) -> Result<PullStats, CentralError> {
    // ref_commit 自己校验名字：这里不必重复一道挡不住任何副作用的守卫。
    let commit = central
        .ref_commit(name)?
        .ok_or_else(|| RepoError::UnknownRef(name.to_string()))?;
    let commit_obj = central.commit_at(&commit)?;
    // 走和包传输同一条路：发"过滤后的树清单"，原树整体不进客户端。
    let list = bundle::manifest(central.store(), commit_obj.tree, prefix)?;
    let mut objects = vec![commit];
    objects.extend(bundle::entries_closure(central.store(), &list.entries)?);
    let transfer = bundle::transfer_scoped(central.store(), client.store(), &objects, &list)?;
    let stats = worktree::materialize(client.store(), &list.id, client.root(), None, force)?;
    Ok(PullStats {
        commit,
        transfer,
        written: stats.written,
        skipped: stats.skipped,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::gate::ClaimSpec;
    use crate::repo::CommitRequest;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trit-vcs-central-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn corpus(dirs: usize, per: usize) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        for d in 0..dirs {
            for f in 0..per {
                out.insert(
                    format!("gen/d{d:02}/f{f}.txt"),
                    format!(
                        "dir {d} file {f} body
"
                    )
                    .into_bytes(),
                );
            }
        }
        out
    }

    fn commit_all(repo: &mut Repo, files: &BTreeMap<String, Vec<u8>>, now: u64) -> Hash {
        repo.stage(files, now).unwrap();
        repo.commit(CommitRequest {
            message: format!("c{now}"),
            session: "qoder:central".to_string(),
            claims: vec![ClaimSpec {
                id: format!("c{now}"),
                owner: "qoder:central".to_string(),
                paths: files.keys().cloned().collect(),
                acquired_at: 1,
                expires_at: u64::MAX / 2,
            }],
            now_secs: now,
            ..Default::default()
        })
        .unwrap()
        .id
    }

    fn open_repo(path: &std::path::Path) -> Repo {
        std::fs::create_dir_all(path).unwrap();
        Repo::init(path).unwrap()
    }

    #[test]
    fn publish_then_pull_prefix_sends_only_that_closure() {
        let root = temp("pull");
        let files = corpus(20, 5);
        let mut src = open_repo(&root.join("src"));
        let commit = commit_all(&mut src, &files, 1_000);

        let mut central = open_repo(&root.join("central"));
        let published = publish(&src, &mut central, "main", None, false).unwrap();
        assert_eq!(published.commit, commit);
        assert!(published.transfer.sent_bytes > 0);
        assert_eq!(central.ref_commit("main").unwrap(), Some(commit));

        let client_dir = root.join("client");
        let mut client = open_repo(&client_dir);
        let pulled = pull(&central, &mut client, "main", Some("gen/d00"), false).unwrap();
        assert!(
            pulled.transfer.sent_bytes * 10 < published.transfer.total_bytes,
            "单目录应远小于全量：{} vs {}",
            pulled.transfer.sent_bytes,
            published.transfer.total_bytes
        );
        assert!(client_dir.join("gen/d00/f0.txt").is_file());
        assert!(!client_dir.join("gen/d01").exists(), "范围外不该被物化");
        assert_eq!(pulled.written, 5, "只该写这个目录的 5 个文件");
        let full_tree = central.commit_at(&commit).unwrap().tree;
        assert!(
            !client.store().has_chunk(&full_tree),
            "整棵原树不该进客户端（否则单目录 = 全量）"
        );

        let again = pull(&central, &mut client, "main", Some("gen/d00"), false).unwrap();
        assert_eq!(again.transfer.sent_bytes, 0, "第二次不该再传内容");
        assert_eq!(again.written, 0, "已一致就不该重写");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pull_refuses_to_overwrite_local_changes() {
        let root = temp("dirty");
        let files = corpus(3, 2);
        let mut src = open_repo(&root.join("src"));
        commit_all(&mut src, &files, 1_000);
        let mut central = open_repo(&root.join("central"));
        publish(&src, &mut central, "main", None, false).unwrap();

        let client_dir = root.join("client");
        let mut client = open_repo(&client_dir);
        pull(&central, &mut client, "main", Some("gen/d00"), false).unwrap();
        std::fs::write(client_dir.join("gen/d00/f0.txt"), b"local edit").unwrap();

        let err = pull(&central, &mut client, "main", Some("gen/d00"), false).unwrap_err();
        assert!(
            matches!(
                err,
                CentralError::Worktree(worktree::WorktreeError::Dirty(..))
            ),
            "{err:?}"
        );
        assert_eq!(
            std::fs::read(client_dir.join("gen/d00/f0.txt")).unwrap(),
            b"local edit",
            "报错时一个字节都不该动"
        );
        let forced = pull(&central, &mut client, "main", Some("gen/d00"), true).unwrap();
        assert_eq!(forced.written, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn refs_are_named_and_listed() {
        let root = temp("refs");
        let mut src = open_repo(&root.join("src"));
        let files = corpus(2, 1);
        let first = commit_all(&mut src, &files, 1_000);
        let mut central = open_repo(&root.join("central"));
        publish(&src, &mut central, "nightly", Some(&first), false).unwrap();
        publish(&src, &mut central, "main", Some(&first), false).unwrap();
        let refs = central.list_refs().unwrap();
        assert_eq!(
            refs.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec!["main", "nightly"]
        );
        let kinds: Vec<&str> = central
            .journal()
            .events()
            .iter()
            .filter(|event| event.kind == "ref")
            .map(|_| "ref")
            .collect();
        assert_eq!(kinds.len(), 2, "每次写 ref 都要进哈希链");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn bad_names_and_missing_refs_are_loud() {
        let root = temp("errors");
        let mut src = open_repo(&root.join("src"));
        let files = corpus(1, 1);
        commit_all(&mut src, &files, 1_000);
        let mut central = open_repo(&root.join("central"));
        for name in ["", "a/b", "..", ".hidden", "a b", &"x".repeat(65)] {
            assert!(
                matches!(
                    publish(&src, &mut central, name, None, false),
                    Err(CentralError::Repo(RepoError::BadRefName(_)))
                ),
                "{name:?} 必须被拒"
            );
        }
        assert_eq!(
            central.store().stats().unwrap().objects,
            0,
            "非法名必须在搬对象之前就被拒（名字坏了不该先往中央仓写块）"
        );
        let mut client = open_repo(&root.join("client"));
        assert!(matches!(
            pull(&central, &mut client, "main", None, false).unwrap_err(),
            CentralError::Repo(RepoError::UnknownRef(_))
        ));
        assert!(
            matches!(
                pull(&central, &mut client, "a/b", None, false).unwrap_err(),
                CentralError::Repo(RepoError::BadRefName(_))
            ),
            "pull 侧的非法名由 ref_commit 挡住，且不能落到别处"
        );
        assert_eq!(
            client.store().stats().unwrap().objects,
            0,
            "被拒的 pull 不该在客户端留下任何对象"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn publish_only_moves_a_ref_forwards() {
        let root = temp("forward");
        let files = corpus(2, 1);
        let mut src = open_repo(&root.join("src"));
        let first = commit_all(&mut src, &files, 1_000);
        let second = commit_all(&mut src, &corpus(2, 2), 1_001);

        // 中央仓是空的：先推 HEAD（second），父链得跟着补齐
        let mut central = open_repo(&root.join("central"));
        publish(&src, &mut central, "main", Some(&second), false).unwrap();
        assert_eq!(central.ref_commit("main").unwrap(), Some(second));
        assert!(
            central.commit_at(&first).is_ok(),
            "父提交对象必须一起搬来，否则中央仓连分叉都判不了"
        );

        // 直接退回父提交 ⇒ 判红，且 ref 还在原位
        let err = publish(&src, &mut central, "main", Some(&first), false).unwrap_err();
        assert!(
            matches!(err, CentralError::Repo(RepoError::NonFastForward { .. })),
            "{err:?}"
        );
        assert_eq!(central.ref_commit("main").unwrap(), Some(second));

        // 另起一条分叉线（父指针强行指回 first）⇒ 同样判红
        let diverged = {
            let other = corpus(1, 1);
            src.stage(&other, 1_005).unwrap();
            src.commit(CommitRequest {
                message: "diverged".to_string(),
                session: "qoder:central".to_string(),
                claims: vec![ClaimSpec {
                    id: "c5".to_string(),
                    owner: "qoder:central".to_string(),
                    paths: other.keys().cloned().collect(),
                    acquired_at: 1,
                    expires_at: u64::MAX / 2,
                }],
                parent_override: Some(first),
                now_secs: 1_005,
                ..Default::default()
            })
            .unwrap()
            .id
        };
        assert!(matches!(
            publish(&src, &mut central, "main", Some(&diverged), false).unwrap_err(),
            CentralError::Repo(RepoError::NonFastForward { .. })
        ));
        assert_eq!(
            central.ref_commit("main").unwrap(),
            Some(second),
            "分叉提交不该把 ref 挪走"
        );

        // 显式改写（--force）才允许
        publish(&src, &mut central, "main", Some(&diverged), true).unwrap();
        assert_eq!(central.ref_commit("main").unwrap(), Some(diverged));
        // 幂等重推同一个提交不算倒退
        publish(&src, &mut central, "main", Some(&diverged), false).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pull_without_prefix_materializes_everything() {
        let root = temp("full");
        let files = corpus(4, 2);
        let mut src = open_repo(&root.join("src"));
        commit_all(&mut src, &files, 1_000);
        let mut central = open_repo(&root.join("central"));
        publish(&src, &mut central, "main", None, false).unwrap();

        let client_dir = root.join("client");
        let mut client = open_repo(&client_dir);
        let pulled = pull(&central, &mut client, "main", None, false).unwrap();
        assert_eq!(pulled.written, 8, "全量 pull 该写满 8 个文件");
        let full_tree = central.commit_at(&pulled.commit).unwrap().tree;
        assert!(
            client.store().has_chunk(&full_tree),
            "不限定范围时清单就是原树，必须落进客户端"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
