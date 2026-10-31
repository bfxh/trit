//! 数据转移桥：把已有项目的历史（Git/GitHub 等）方便地转移进免疫系统。
//!
//! 方向一 **转入**（import）：`git log` → [`CommitRecord`] 流 → 每条 commit
//! 在 `parent` 下立一个 `Message` 节点。**防污染协议**：导入的历史是"事实"
//! 不是"裁决"——作者写下过什么、何时写的，与"这段代码是否已验证"无关，
//! 因此一律 **0 态入账**（可选自动隔离），绝不从提交信息推断 ±1。
//!
//! 方向二 **转出**（export）：DAG 全量 → TSV（[`export_dag_tsv`]），
//! git-friendly（可 diff、可提交、任何表格工具可读）。
//!
//! [`collect_git_log`] 是对 `git` CLI 的便捷封装（外部工具而非依赖；参数
//! 为固定字面量，无注入面；仓库路径必须是存在的目录）。解析器
//! [`parse_git_log`] 本身是纯函数，可直接对接 GitHub API 导出的等价文本。

use crate::journal::Journal;
use std::io;
use std::path::Path;
use std::process::Command;

/// 一条提交记录（`git log` 的最小骨架）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRecord {
    pub hash: String,
    pub author: String,
    /// 作者时间（Unix 毫秒）。
    pub timestamp_ms: u64,
    pub subject: String,
}

/// 记录/字段分隔符（与 `--pretty=format:%x1e%H%x1f%an%x1f%at%x1f%s` 对应）。
const RECORD_SEP: char = '\u{1e}';
const FIELD_SEP: char = '\u{1f}';

/// `git log --pretty=format:...` 输出的纯文本解析。宽容：坏行跳过、空行忽略。
pub fn parse_git_log(text: &str) -> Vec<CommitRecord> {
    let mut out = Vec::new();
    for record in text.split(RECORD_SEP) {
        let record = record.trim_start_matches('\n').trim_end_matches('\n');
        if record.is_empty() {
            continue;
        }
        let mut fields = record.split(FIELD_SEP);
        let (Some(hash), Some(author), Some(at), Some(subject)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (hash, author, subject) =
            (hash.trim(), author.trim(), subject.trim());
        if hash.is_empty() || author.is_empty() {
            continue;
        }
        let Ok(secs) = at.trim().parse::<u64>() else {
            continue;
        };
        out.push(CommitRecord {
            hash: hash.to_string(),
            author: author.to_string(),
            timestamp_ms: secs * 1000,
            subject: subject.to_string(),
        });
    }
    out
}

/// 便捷封装：对 `repo` 目录跑 `git log`，返回解析好的提交流。
/// 新→旧顺序（git 默认）。失败（非 git 目录 / git 不可用）如实报错。
pub fn collect_git_log(repo: &Path) -> io::Result<Vec<CommitRecord>> {
    if !repo.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{repo:?} 不是目录"),
        ));
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("log")
        .arg("--pretty=format:%x1e%H%x1f%an%x1f%at%x1f%s")
        .output()?;
    if !out.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(parse_git_log(&String::from_utf8_lossy(&out.stdout)))
}

/// 导入提交历史：每条 commit 在 `parent` 下立 `Message` 节点，**一律 0 态**
/// （历史是事实不是裁决）；`quarantine=true` 时整批自动隔离（外部仓库默认），
/// 出批等人放行。返回立账节点数。
pub fn import_commits(
    journal: &mut Journal,
    parent: crate::NodeId,
    commits: &[CommitRecord],
    quarantine: bool,
) -> io::Result<usize> {
    let mut n = 0usize;
    for c in commits {
        let short: String = c.hash.chars().take(12).collect();
        let node = journal
            .spawn(
                parent,
                crate::NodeKind::Message,
                &format!("commit {short} {}", c.subject),
            )
            .map_err(io::Error::other)?;
        // 0 态入账：不写 verdict（新节点默认 Unknown），历史只进 note
        let note = format!(
            "author={} at={}（导入历史 0 态：作者声称≠已验证）",
            c.author, c.timestamp_ms
        );
        if quarantine {
            journal
                .quarantine(node, &format!("外部导入自动隔离 {note}"))
                .map_err(io::Error::other)?;
        } else {
            journal.record(node, crate::Trit::Unknown, None, &note).map_err(io::Error::other)?;
        }
        n += 1;
    }
    Ok(n)
}

/// 导出：DAG 全量 → TSV——实现已归位 [`crate::journal`]（内核：账本导出
/// 是 journal 域职责，attest 等内核模块依赖它），此处保留路径兼容既有调用方。
pub use crate::journal::export_dag_tsv;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dag::ImmuneDag;

    #[test]
    fn parse_git_log_roundtrip() {
        let text = "\u{1e}abc123def456\u{1f}alice\u{1f}1700000000\u{1f}feat: 登录模块\n\u{1e}fff000\u{1f}bob\u{1f}1700000100\u{1f}fix: 越界\n";
        let commits = parse_git_log(text);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "abc123def456");
        assert_eq!(commits[0].author, "alice");
        assert_eq!(commits[0].timestamp_ms, 1_700_000_000_000);
        assert_eq!(commits[1].subject, "fix: 越界");
        // 坏行跳过
        assert_eq!(parse_git_log("垃圾行"), Vec::new());
        assert_eq!(parse_git_log(""), Vec::new());
    }

    #[test]
    fn import_is_zero_state_and_quarantinable() {
        let p = std::env::temp_dir().join(format!("trit-import-{}.journal", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut j = Journal::create_as(&p, "import", 0).unwrap();
        let commits = vec![
            CommitRecord { hash: "a".repeat(40), author: "alice".into(), timestamp_ms: 1, subject: "feat: x".into() },
            CommitRecord { hash: "b".repeat(40), author: "bob".into(), timestamp_ms: 2, subject: "fix: y".into() },
        ];
        // 外部仓库 → 隔离
        let n = import_commits(&mut j, crate::NodeId(0), &commits, true).unwrap();
        assert_eq!(n, 2);
        let dag = j.dag().clone();
        let msgs: Vec<_> = dag.nodes().iter().filter(|nd| nd.kind == crate::NodeKind::Message).collect();
        assert_eq!(msgs.len(), 2);
        assert!(msgs.iter().all(|nd| nd.verdict == crate::Trit::Unknown), "导入历史一律 0 态");
        assert!(msgs.iter().all(|nd| nd.quarantined), "外部导入应隔离");
        drop(j.into_dag().unwrap());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn export_tsv_shape() {
        let mut dag = ImmuneDag::root("x");
        let n = dag.spawn(crate::NodeId(0), crate::NodeKind::AttackVector, "SQLi\t注入");
        dag.record(n, crate::Trit::Vulnerable, None, "打穿");
        let tsv = export_dag_tsv(&dag);
        let lines: Vec<&str> = tsv.lines().collect();
        assert_eq!(lines[0], "# trit-immune dag export v1");
        // 根 + spawn，列数恒 6；title 内制表符已被替换
        let data: Vec<Vec<&str>> = lines[2..].iter().map(|l| l.split('\t').collect()).collect();
        assert_eq!(data.len(), 2);
        assert!(data.iter().all(|row| row.len() == 6), "恒 6 列");
        assert!(data[1][5].contains("SQLi 注入"), "制表符已替换");
    }
}
