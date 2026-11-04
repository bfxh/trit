//! 三方合并 v0：行级三路 + **结构化争用对象**。
//!
//! 与 git 的差别不在"会不会冲突"，在**冲突以什么形式交给下一个人/智能体**：
//! 这里不写 `<<<<<<<` 标记，而是产出 `{path, kind, base/ours/theirs 对象 id, hunks, raw_bytes}`，
//! 每一处争用只带冲突窗口内的行，不要求读整文件。伪冲突（只差行尾空白）直接归并，不报冲突。
//!
//! v0 只做文本行级；Rust/Python 结构级与"错解率=0"的等价门在后续版本，不在这里宣称。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::hash::Hash;
use crate::repo::{Repo, RepoError};
use crate::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("仓库: {0}")]
    Repo(#[from] RepoError),
    #[error("对象库: {0}")]
    Store(#[from] StoreError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    Content,
    Binary,
    DeleteModify,
    AddAdd,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictHunk {
    pub base_start: usize,
    pub base_lines: Vec<String>,
    pub ours: Vec<String>,
    pub theirs: Vec<String>,
}

/// 一处争用：只带窗口内的行，不带整文件。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    pub kind: ConflictKind,
    /// 文件对象 id（不是内容）：需要全文时按 id 取。
    pub base: Option<String>,
    pub ours: Option<String>,
    pub theirs: Option<String>,
    #[serde(default)]
    pub hunks: Vec<ConflictHunk>,
    /// 两侧原文总字节数（"读整文件"的成本基线）。
    pub raw_bytes: u64,
}

impl Conflict {
    pub fn encode(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("争用对象只含可序列化字段")
    }

    /// 结构化对象的字节数（token 成本表用；不含压缩）。
    pub fn object_bytes(&self) -> u64 {
        self.encode().len() as u64
    }

    pub fn id(&self) -> Hash {
        Hash::of(&self.encode())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeOutcome {
    /// 无冲突时的合并树；有冲突时为 `None`（不给"半棵树"让人误提交）。
    pub tree: Option<Hash>,
    pub conflicts: Vec<Conflict>,
    pub merged_files: usize,
    pub removed_files: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Hunk {
    start: usize,
    len: usize,
    lines: Vec<String>,
}

enum TextMerge {
    Clean(Vec<String>),
    Conflict(Vec<ConflictHunk>),
}

fn split_lines(text: &str) -> Vec<String> {
    text.split('\n').map(str::to_string).collect()
}

fn normalize(lines: &[String]) -> Vec<&str> {
    lines.iter().map(|line| line.trim_end()).collect()
}

/// base → side 的编辑块（LCS；超大文件退化成"整文件替换"一块）。
fn diff_hunks(base: &[String], side: &[String]) -> Vec<Hunk> {
    let (n, m) = (base.len(), side.len());
    if n.saturating_mul(m) > 4_000_000 {
        return vec![Hunk {
            start: 0,
            len: n,
            lines: side.to_vec(),
        }];
    }
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if base[i] == side[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut hunks = Vec::new();
    while i < n || j < m {
        if i < n && j < m && base[i] == side[j] {
            i += 1;
            j += 1;
            continue;
        }
        let start = i;
        let mut lines = Vec::new();
        loop {
            let take_side = j < m && (i == n || dp[i][j + 1] >= dp[i + 1][j]);
            if take_side {
                lines.push(side[j].clone());
                j += 1;
            } else {
                i += 1;
            }
            if i < n && j < m && base[i] == side[j] {
                break;
            }
            if i >= n && j >= m {
                break;
            }
        }
        hunks.push(Hunk {
            start,
            len: i - start,
            lines,
        });
    }
    hunks
}

fn overlaps(a: &Hunk, b: &Hunk) -> bool {
    let a_end = a.start + a.len;
    let b_end = b.start + b.len;
    if a.start == b.start {
        return true;
    }
    if a.len == 0 {
        return a.start > b.start && a.start < b_end;
    }
    if b.len == 0 {
        return b.start > a.start && b.start < a_end;
    }
    a.start < b_end && b.start < a_end
}

fn apply_group(base: &[String], hunks: &[&Hunk], start: usize, end: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = start;
    for hunk in hunks {
        if cursor < hunk.start {
            out.extend_from_slice(&base[cursor..hunk.start]);
        }
        out.extend(hunk.lines.iter().cloned());
        cursor = cursor.max(hunk.start + hunk.len);
    }
    if cursor < end {
        out.extend_from_slice(&base[cursor..end]);
    }
    out
}

fn merge_text(base: &[String], ours: &[String], theirs: &[String]) -> TextMerge {
    if ours == theirs {
        return TextMerge::Clean(ours.to_vec());
    }
    if ours == base {
        return TextMerge::Clean(theirs.to_vec());
    }
    if theirs == base {
        return TextMerge::Clean(ours.to_vec());
    }
    // 伪冲突：两侧只差行尾空白 ⇒ 取 ours，不报冲突。
    if normalize(ours) == normalize(theirs) {
        return TextMerge::Clean(ours.to_vec());
    }
    // 只有一侧做了格式化（相对 base 只差行尾空白）⇒ 取另一侧，格式噪音不挡路。
    if normalize(ours) == normalize(base) {
        return TextMerge::Clean(theirs.to_vec());
    }
    if normalize(theirs) == normalize(base) {
        return TextMerge::Clean(ours.to_vec());
    }

    let ours_hunks = diff_hunks(base, ours);
    let theirs_hunks = diff_hunks(base, theirs);
    if ours_hunks.is_empty() {
        return TextMerge::Clean(theirs.to_vec());
    }
    if theirs_hunks.is_empty() {
        return TextMerge::Clean(ours.to_vec());
    }

    let mut tagged: Vec<(bool, Hunk)> = Vec::new();
    tagged.extend(ours_hunks.into_iter().map(|hunk| (true, hunk)));
    tagged.extend(theirs_hunks.into_iter().map(|hunk| (false, hunk)));
    tagged.sort_by_key(|(_, hunk)| (hunk.start, hunk.len));

    let mut groups: Vec<Vec<usize>> = Vec::new();
    for index in 0..tagged.len() {
        let joined = groups.last().is_some_and(|group| {
            group
                .iter()
                .any(|other| overlaps(&tagged[*other].1, &tagged[index].1))
        });
        if joined {
            groups.last_mut().unwrap().push(index);
        } else {
            groups.push(vec![index]);
        }
    }

    let mut out = Vec::new();
    let mut cursor = 0usize;
    let mut conflicts = Vec::new();
    for group in groups {
        let start = group.iter().map(|i| tagged[*i].1.start).min().unwrap();
        let end = group
            .iter()
            .map(|i| tagged[*i].1.start + tagged[*i].1.len)
            .max()
            .unwrap();
        if cursor < start {
            out.extend_from_slice(&base[cursor..start]);
        }
        let group_hunks: Vec<&(bool, Hunk)> = group.iter().map(|i| &tagged[*i]).collect();
        let ours_part: Vec<&Hunk> = group_hunks
            .iter()
            .filter(|(is_ours, _)| *is_ours)
            .map(|(_, hunk)| hunk)
            .collect();
        let theirs_part: Vec<&Hunk> = group_hunks
            .iter()
            .filter(|(is_ours, _)| !*is_ours)
            .map(|(_, hunk)| hunk)
            .collect();
        let only_ours = theirs_part.is_empty();
        let only_theirs = ours_part.is_empty();
        let ours_out = if only_theirs {
            base[start..end].to_vec()
        } else {
            apply_group(base, &ours_part, start, end)
        };
        let theirs_out = if only_ours {
            base[start..end].to_vec()
        } else {
            apply_group(base, &theirs_part, start, end)
        };
        if only_theirs {
            out.extend(theirs_out);
        } else if only_ours
            || ours_out == theirs_out
            || normalize(&ours_out) == normalize(&theirs_out)
        {
            out.extend(ours_out);
        } else {
            conflicts.push(ConflictHunk {
                base_start: start,
                base_lines: base[start..end].to_vec(),
                ours: ours_out,
                theirs: theirs_out,
            });
        }
        cursor = end;
    }
    if cursor < base.len() {
        out.extend_from_slice(&base[cursor..]);
    }
    if conflicts.is_empty() {
        TextMerge::Clean(out)
    } else {
        TextMerge::Conflict(conflicts)
    }
}

enum FileMerge {
    Clean(Vec<u8>),
    Conflict {
        kind: ConflictKind,
        hunks: Vec<ConflictHunk>,
    },
}

fn merge_file_bytes(base: Option<&[u8]>, ours: &[u8], theirs: &[u8]) -> FileMerge {
    let binary = |bytes: &[u8]| bytes.contains(&0);
    if binary(ours) || binary(theirs) || base.is_some_and(binary) {
        return FileMerge::Conflict {
            kind: ConflictKind::Binary,
            hunks: Vec::new(),
        };
    }
    let (Ok(base_text), Ok(ours_text), Ok(theirs_text)) = (
        std::str::from_utf8(base.unwrap_or(b"")),
        std::str::from_utf8(ours),
        std::str::from_utf8(theirs),
    ) else {
        return FileMerge::Conflict {
            kind: ConflictKind::Binary,
            hunks: Vec::new(),
        };
    };
    let base_lines = split_lines(base_text);
    let ours_lines = split_lines(ours_text);
    let theirs_lines = split_lines(theirs_text);
    match merge_text(&base_lines, &ours_lines, &theirs_lines) {
        TextMerge::Clean(lines) => FileMerge::Clean(lines.join("\n").into_bytes()),
        TextMerge::Conflict(hunks) => FileMerge::Conflict {
            kind: ConflictKind::Content,
            hunks,
        },
    }
}

fn file_size(repo: &Repo, id: Option<Hash>) -> Result<u64, MergeError> {
    match id {
        Some(id) => Ok(repo.store().file_map(&id)?.size),
        None => Ok(0),
    }
}

fn object_hex(id: Option<Hash>) -> Option<String> {
    id.map(|hash| hash.to_hex())
}

/// 三方合并两个提交（`base` 为 `None` 时自动找第一父链的公共祖先）。
pub fn merge(
    repo: &Repo,
    ours: &Hash,
    theirs: &Hash,
    base: Option<&Hash>,
) -> Result<MergeOutcome, MergeError> {
    let base_commit = match base {
        Some(id) => Some(*id),
        None => repo.merge_base(ours, theirs)?,
    };
    let ours_entries = repo.store().tree_entries(&repo.commit_at(ours)?.tree)?;
    let theirs_entries = repo.store().tree_entries(&repo.commit_at(theirs)?.tree)?;
    let base_entries = match base_commit {
        Some(id) => repo.store().tree_entries(&repo.commit_at(&id)?.tree)?,
        None => BTreeMap::new(),
    };

    let mut paths: BTreeSet<String> = BTreeSet::new();
    paths.extend(ours_entries.keys().cloned());
    paths.extend(theirs_entries.keys().cloned());
    paths.extend(base_entries.keys().cloned());

    let mut merged_entries = BTreeMap::new();
    let mut conflicts = Vec::new();
    let mut merged_files = 0usize;
    let mut removed_files = 0usize;

    for path in paths {
        let base_id = base_entries.get(&path).copied();
        let ours_id = ours_entries.get(&path).copied();
        let theirs_id = theirs_entries.get(&path).copied();

        if let (Some(o), Some(t)) = (ours_id, theirs_id)
            && o == t
        {
            merged_entries.insert(path, o);
            merged_files += 1;
            continue;
        }
        if let Some(t) = theirs_id
            && ours_id == base_id
        {
            merged_entries.insert(path, t);
            merged_files += 1;
            continue;
        }
        if let Some(o) = ours_id
            && theirs_id == base_id
        {
            merged_entries.insert(path, o);
            merged_files += 1;
            continue;
        }

        // 删除 vs 修改（含删除 vs 未变）：只有和 base 不同的一侧算争用。
        if base_id.is_some() && (ours_id.is_none() || theirs_id.is_none()) {
            let changed_id = ours_id.or(theirs_id);
            if changed_id == base_id {
                removed_files += 1;
                continue;
            }
            conflicts.push(Conflict {
                path,
                kind: ConflictKind::DeleteModify,
                base: object_hex(base_id),
                ours: object_hex(ours_id),
                theirs: object_hex(theirs_id),
                hunks: Vec::new(),
                raw_bytes: file_size(repo, base_id)?
                    + file_size(repo, ours_id)?
                    + file_size(repo, theirs_id)?,
            });
            continue;
        }

        let (Some(ours_id), Some(theirs_id)) = (ours_id, theirs_id) else {
            // 单边新增：直接收。
            let only = ours_id.or(theirs_id).unwrap();
            merged_entries.insert(path, only);
            merged_files += 1;
            continue;
        };

        let base_bytes = match base_id {
            Some(id) => Some(repo.store().get_file(&id)?),
            None => None,
        };
        let ours_bytes = repo.store().get_file(&ours_id)?;
        let theirs_bytes = repo.store().get_file(&theirs_id)?;
        let raw_bytes = base_bytes.as_ref().map_or(0, |b| b.len() as u64)
            + ours_bytes.len() as u64
            + theirs_bytes.len() as u64;
        match merge_file_bytes(base_bytes.as_deref(), &ours_bytes, &theirs_bytes) {
            FileMerge::Clean(bytes) => {
                let (id, _) = repo.store().put_file(&bytes)?;
                merged_entries.insert(path, id);
                merged_files += 1;
            }
            FileMerge::Conflict { kind, hunks } => {
                let kind = if base_id.is_none() {
                    ConflictKind::AddAdd
                } else {
                    kind
                };
                conflicts.push(Conflict {
                    path,
                    kind,
                    base: object_hex(base_id),
                    ours: object_hex(Some(ours_id)),
                    theirs: object_hex(Some(theirs_id)),
                    hunks,
                    raw_bytes,
                });
            }
        }
    }

    let tree = if conflicts.is_empty() {
        let encoded = serde_json::to_vec(&merged_entries)
            .map_err(|e| MergeError::Store(StoreError::BadMap(e.to_string())))?;
        let (id, _) = repo.store().put_chunk(&encoded)?;
        Some(id)
    } else {
        None
    };
    for conflict in &conflicts {
        repo.store().put_chunk(&conflict.encode())?;
    }
    Ok(MergeOutcome {
        tree,
        conflicts,
        merged_files,
        removed_files,
    })
}

#[cfg(test)]
mod tests;
