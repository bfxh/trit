//! `trit` 命令行：git 风格操作面——**journal 代替 git** 的入口。
//!
//! git 的每个核心操作在免疫账本上都有对应物（且大多更强）：
//!
//! | git | trit | 关系 |
//! |-----|------|------|
//! | `git init` | `trit init` | Journal::create（追加式日志即数据库） |
//! | `git log` | `trit log` | HLC 全序的 Delta 历史 |
//! | `git status` | `trit status` | 收敛度量 + 可疑队列 + 根门禁 |
//! | `git branch` | `trit branch` | spawn 分支节点（DAG 天生分叉） |
//! | `git commit` | `trit commit` | 记录裁决事件（新节点默认 0：未测不可信） |
//! | `git merge` | `trit merge` | merge_replicas（δ-CRDT：**无冲突合并**，与顺序无关） |
//! | `git checkout` | `trit checkout` | 重放到任意节点 = 时间旅行（日志不可变，历史永不丢） |
//! | `git clone` | `trit clone` | 复制 journal 文件（账本即工件） |
//!
//! 超出 git 的部分：`trit gate`（三值门禁——git 只知道冲突，不知道风险）、
//! `trit suspects`（0 态待办队列）。
//!
//! 默认账本：当前目录 `repo.tritj`（`-f` 可指定）。

use std::path::PathBuf;
use std::process::ExitCode;

use trit_immune::{delta, parse_stamped_line, Journal, NodeId, NodeKind, Trit};

const DEFAULT_LEDGER: &str = "repo.tritj";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(CliError::Usage(msg)) => {
            eprintln!("用法错误：{msg}\n\n{}", help_text());
            ExitCode::from(2)
        }
        Err(CliError::Runtime(msg)) => {
            eprintln!("错误：{msg}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Runtime(String),
}

impl From<std::io::Error> for CliError {
    fn from(e: std::io::Error) -> Self {
        Self::Runtime(e.to_string())
    }
}

impl From<JournalErrorAlias> for CliError {
    fn from(e: JournalErrorAlias) -> Self {
        Self::Runtime(e.to_string())
    }
}

type JournalErrorAlias = trit_immune::JournalError;

fn run(args: &[String]) -> Result<(), CliError> {
    let Some(cmd) = args.first() else {
        return Err(CliError::Usage("缺命令".into()));
    };
    let rest = &args[1..];
    match cmd.as_str() {
        "init" => cmd_init(rest),
        "log" => cmd_log(rest),
        "status" => cmd_status(rest),
        "branch" => cmd_branch(rest),
        "commit" => cmd_commit(rest),
        "merge" => cmd_merge(rest),
        "checkout" => cmd_checkout(rest),
        "clone" => cmd_clone(rest),
        "gate" => cmd_gate(rest),
        "suspects" => cmd_suspects(rest),
        "sync-git" => cmd_sync_git(rest),
        "help" | "--help" | "-h" => {
            println!("{}", help_text());
            Ok(())
        }
        other => Err(CliError::Usage(format!("未知命令 {other:?}"))),
    }
}

// ── 参数工具 ────────────────────────────────────────────────────────────

/// 从参数里取 `-f <file>`（默认 repo.tritj），返回剩余位置参数。
fn split_ledger(args: &[String]) -> (PathBuf, &[String]) {
    if args.len() >= 2 && args[0] == "-f" {
        (PathBuf::from(&args[1]), &args[2..])
    } else {
        (PathBuf::from(DEFAULT_LEDGER), args)
    }
}

fn open_ledger(args: &[String]) -> Result<(Journal, PathBuf), CliError> {
    let (path, _) = split_ledger(args);
    let j = Journal::open_as(&path, 0)
        .map_err(|e| CliError::Runtime(format!("打开账本 {} 失败：{e}", path.display())))?;
    Ok((j, path))
}

// ── 命令 ────────────────────────────────────────────────────────────────

/// `trit init <dir> [title]`：在目录下建新账本（`<dir>/repo.tritj`）。
fn cmd_init(args: &[String]) -> Result<(), CliError> {
    let dir = args
        .first()
        .ok_or_else(|| CliError::Usage("init 需要 <dir>".into()))?;
    let title = args.get(1).cloned().unwrap_or_else(|| "main".into());
    let path = PathBuf::from(dir).join(DEFAULT_LEDGER);
    if path.exists() {
        return Err(CliError::Runtime(format!("{} 已存在", path.display())));
    }
    Journal::create_as(&path, &title, 0)?;
    println!("初始化账本 {}（根：{title}）", path.display());
    Ok(())
}

/// `trit log [-n N] [-f file]`：HLC 全序的 Delta 历史。
fn cmd_log(args: &[String]) -> Result<(), CliError> {
    // 独立扫描：-n 与 -f 任意顺序
    let mut limit = usize::MAX;
    let mut path = PathBuf::from(DEFAULT_LEDGER);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => {
                limit = args
                    .get(i + 1)
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(usize::MAX);
                i += 2;
            }
            "-f" => {
                if let Some(p) = args.get(i + 1) {
                    path = PathBuf::from(p);
                }
                i += 2;
            }
            _ => i += 1,
        }
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CliError::Runtime(format!("读账本失败：{e}")))?;
    let mut shown = 0usize;
    for (i, line) in text.lines().enumerate() {
        if i == 0 {
            continue; // 头部
        }
        if line.trim().is_empty() {
            continue;
        }
        if shown >= limit {
            break;
        }
        let Ok((ts, d)) = parse_stamped_line(line) else {
            println!("{:4}: <坏行如实展示> {line}", i);
            continue;
        };
        println!("{:4} {} {}", i, ts, delta::encode(&d));
        shown += 1;
    }
    Ok(())
}

/// `trit status [-f file]`：收敛度量 + 根门禁 + 节点数。
fn cmd_status(args: &[String]) -> Result<(), CliError> {
    let (j, path) = open_ledger(args)?;
    let dag = j.dag().clone();
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    let conv = dag.convergence();
    let root_gate = dag.merge_gate(NodeId((dag.nodes().len() - 1) as u32));
    println!("账本：{}", path.display());
    println!(
        "节点：{}（+{} / 0:{} / -{} / 隔离 {}）",
        conv.total, conv.hardened, conv.unknown, conv.vulnerable, conv.quarantined
    );
    println!("收敛率：{:.3}", conv.ratio);
    println!("尾门禁：{root_gate:?}");
    let suspects = dag.suspects();
    println!("可疑队列：{} 个", suspects.len());
    Ok(())
}

/// `trit branch <name> [-f file]`：spawn 分支节点（DAG 天生分叉，无 HEAD 概念——
/// 每个分支就是一个节点，工作目标用节点号寻址）。
fn cmd_branch(args: &[String]) -> Result<(), CliError> {
    let (name, ledger_args) = args
        .split_first()
        .ok_or_else(|| CliError::Usage("branch 需要 <name>".into()))?;
    let (mut j, path) = open_ledger(ledger_args)?;
    let parent = NodeId((j.dag().nodes().len() - 1) as u32);
    let id = j.spawn(parent, NodeKind::Delta, &format!("branch:{name}"))?;
    let n = id.0;
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    println!("分支 {name} = 节点 #{n:02}（账本 {}）", path.display());
    Ok(())
}

/// `trit commit -m <note> [-v -1|0|+1] [-n <node>] [-f file]`：
/// 记录裁决事件。默认 0 态（未测不可信）；`-n` 缺省 = 最新节点。
fn cmd_commit(args: &[String]) -> Result<(), CliError> {
    let mut note: Option<String> = None;
    let mut verdict = Trit::Unknown;
    let mut node: Option<u32> = None;
    let mut ledger_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-m" => {
                note = args.get(i + 1).cloned();
                i += 2;
            }
            "-v" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| CliError::Usage("-v 需要 -1|0|+1".into()))?;
                verdict = v
                    .parse()
                    .map_err(|_| CliError::Usage(format!("非法裁决 {v:?}（-1|0|+1）")))?;
                i += 2;
            }
            "-n" => {
                let n = args
                    .get(i + 1)
                    .ok_or_else(|| CliError::Usage("-n 需要节点号".into()))?;
                node = Some(
                    n.parse()
                        .map_err(|_| CliError::Usage(format!("节点号非数字：{n:?}")))?,
                );
                i += 2;
            }
            other => {
                ledger_args.push(other.to_string());
                i += 1;
            }
        }
    }
    let note = note.ok_or_else(|| CliError::Usage("commit 需要 -m <note>".into()))?;
    let (mut j, path) = open_ledger(&ledger_args)?;
    let target = match node {
        Some(n) => NodeId(n),
        None => NodeId((j.dag().nodes().len() - 1) as u32),
    };
    if target.0 as usize >= j.dag().nodes().len() {
        return Err(CliError::Runtime(format!("节点 #{:02} 不存在", target.0)));
    }
    j.record(target, verdict, None, &note)?;
    let n = target.0;
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    println!("记录 {n:02} ← {verdict}（{}）", path.display());
    Ok(())
}

/// `trit merge <other> [-f file]`：δ-CRDT 合并——HLC 戳去重、全序归并、
/// **与输入顺序无关、无冲突**（这是比 git merge 强的地方：没有冲突标记，
/// 只有收敛）。
fn cmd_merge(args: &[String]) -> Result<(), CliError> {
    let (other, rest) = args
        .split_first()
        .ok_or_else(|| CliError::Usage("merge 需要 <other 账本>".into()))?;
    let (mut local, path) = open_ledger(rest)?;
    let other_path = PathBuf::from(other);
    if !other_path.is_file() {
        return Err(CliError::Runtime(format!(
            "{} 不是文件",
            other_path.display()
        )));
    }
    // 读入对方日志（跳过头部），以写者 id=1 的身份参与合并
    let text = std::fs::read_to_string(&other_path)
        .map_err(|e| CliError::Runtime(format!("读 {} 失败：{e}", other_path.display())))?;
    let mut remote: Vec<trit_immune::Stamped> = Vec::new();
    for line in text.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let (ts, d) = parse_stamped_line(line)
            .map_err(|e| CliError::Runtime(format!("{} 坏行：{e}", other_path.display())))?;
        remote.push(trit_immune::Stamped { ts, delta: d });
    }
    let local_stream: Vec<trit_immune::Stamped> = {
        let ltext = std::fs::read_to_string(&path)
            .map_err(|e| CliError::Runtime(format!("读 {} 失败：{e}", path.display())))?;
        ltext
            .lines()
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| parse_stamped_line(l).map(|(ts, d)| trit_immune::Stamped { ts, delta: d }))
            .collect::<Result<Vec<_>, _>>()
            .map_err(CliError::Runtime)?
    };
    let outcome = trit_immune::merge_replicas(vec![(0, local_stream), (1, remote)]);
    if !outcome.deferred.is_empty() {
        return Err(CliError::Runtime(format!(
            "{} 条增量进隔离区（重放失败，见 journal 文档）",
            outcome.deferred.len()
        )));
    }
    local.adopt(outcome.canonical)?;
    println!(
        "合并完成：{} 节点（收敛自两份账本，与顺序无关）",
        local.dag().nodes().len()
    );
    Ok(())
}

/// `trit checkout <node> [-f file]`：**时间旅行**——重放到节点 N 时的全图
/// 状态（只读视图；日志不可变，历史永不丢，这正是账本代替 git 的底气）。
fn cmd_checkout(args: &[String]) -> Result<(), CliError> {
    let (n, ledger_args) = args
        .split_first()
        .ok_or_else(|| CliError::Usage("checkout 需要 <node>".into()))?;
    let n: u32 = n
        .parse()
        .map_err(|_| CliError::Usage(format!("节点号非数字：{n:?}")))?;
    let (j, _) = open_ledger(ledger_args)?;
    let dag = j.dag().clone();
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    if n as usize >= dag.nodes().len() {
        return Err(CliError::Runtime(format!("节点 #{n:02} 不存在")));
    }
    // 重放：只保留 n0..=n（含父链闭包），打印时间旅行视图
    let mut view = trit_immune::ImmuneDag::root("时间旅行视图");
    for node in &dag.nodes()[..=n as usize] {
        let kind = node.kind;
        let parent = node
            .parent
            .map(|p| NodeId(p.0.min(view.nodes().len() as u32 - 1)))
            .unwrap_or(NodeId(0));
        let id = view.spawn(parent, kind, &node.title);
        if node.quarantined {
            view.quarantine(id, "checkout：保留隔离标记");
        }
        for e in &node.history {
            view.record(id, e.verdict, None, &e.note);
        }
    }
    println!("节点 #{n:02} 时的全图（{} 节点）：", view.nodes().len());
    print!("{}", view.render());
    Ok(())
}

/// `trit clone <src> <dst>`：账本即工件——复制文件即克隆（含全部历史）。
fn cmd_clone(args: &[String]) -> Result<(), CliError> {
    let (src, dst) = args
        .split_first()
        .and_then(|(s, r)| r.first().map(|d| (s, d)))
        .ok_or_else(|| CliError::Usage("clone 需要 <src> <dst>".into()))?;
    std::fs::copy(src, dst).map_err(|e| CliError::Runtime(format!("克隆失败：{e}")))?;
    println!("克隆 {src} → {dst}（含全部历史，追加式日志天然完整）");
    Ok(())
}

/// `trit gate [-n <node>] [-f file]`：三值门禁——git 只知道冲突，这里知道风险。
fn cmd_gate(args: &[String]) -> Result<(), CliError> {
    let (j, _) = open_ledger(args)?;
    let dag = j.dag().clone();
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    let tip = NodeId((dag.nodes().len() - 1) as u32);
    let gate = dag.merge_gate(tip);
    let breaches = dag.breach_sources(tip);
    println!("尾节点 #{:02} 门禁：{gate:?}", tip.0);
    if breaches.is_empty() {
        println!("无打穿来源");
    } else {
        println!("打穿来源：");
        for (id, kind) in breaches {
            println!("  #{:02}（{:?}）", id.0, kind);
        }
    }
    Ok(())
}

/// `trit suspects [-f file]`：0 态待办队列（未测即不可信的名单）。
fn cmd_suspects(args: &[String]) -> Result<(), CliError> {
    let (j, _) = open_ledger(args)?;
    let dag = j.dag().clone();
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    let suspects = dag.suspects();
    println!("可疑队列 {} 个：", suspects.len());
    for id in suspects {
        let n = dag.node(id);
        println!("  #{:02} {}", id.0, n.title);
    }
    Ok(())
}

/// `trit sync-git <repo> [-f file] [--open]`：**git 共存桥**——git 继续管
/// 代码文件（内容寻址、生态工具），账本把 git 提交历史吸收进来参与裁决。
/// 每条 commit 立一个 Message 节点：**0 态入账**（作者声称≠已验证），
/// 默认整批隔离（外部导入防污染协议；`--open` 解除隔离等人逐条放行）。
/// 返回账本后，扫描器/红队/门禁就可以指着这些节点干活。
fn cmd_sync_git(args: &[String]) -> Result<(), CliError> {
    let mut quarantine = true;
    let mut rest: Vec<String> = Vec::new();
    for a in args {
        match a.as_str() {
            "--open" => quarantine = false,
            other => rest.push(other.to_string()),
        }
    }
    let (repo, ledger_args) = rest
        .split_first()
        .ok_or_else(|| CliError::Usage("sync-git 需要 <repo 路径>".into()))?;
    let repo_path = PathBuf::from(repo);
    let (mut j, path) = open_ledger(ledger_args)?;
    let commits = trit_immune::collect_git_log(&repo_path)
        .map_err(|e| CliError::Runtime(format!("读 git 历史失败：{e}")))?;
    let parent = NodeId((j.dag().nodes().len() - 1) as u32);
    let n = trit_immune::import_commits(&mut j, parent, &commits, quarantine)?;
    drop(j.into_dag().map_err(|e| CliError::Runtime(e.to_string()))?);
    println!(
        "导入 {} 条提交 → {} 个 0 态节点（隔离={}，账本 {}）",
        commits.len(),
        n,
        quarantine,
        path.display()
    );
    Ok(())
}

fn help_text() -> String {
    "trit —— journal 代替 git 的命令行（三进制免疫账本）

命令：
  init <dir> [title]        建新账本（<dir>/repo.tritj）
  log [-n N] [-f file]      HLC 全序 Delta 历史
  status [-f file]          收敛度量 + 尾门禁 + 可疑数
  branch <name> [-f file]   spawn 分支节点（DAG 天生分叉）
  commit -m <note> [-v -1|0|+1] [-n <node>] [-f file]
                            记录裁决事件（默认 0：未测不可信）
  merge <other> [-f file]   δ-CRDT 合并（无冲突，与顺序无关）
  checkout <node> [-f file] 时间旅行：重放到节点 N 的全图状态
  clone <src> <dst>         账本即工件，复制即克隆
  gate [-n node] [-f file]  三值门禁 + 打穿来源
  suspects [-f file]        0 态待办队列
  sync-git <repo> [-f file] [--open]
                            git 共存桥：提交历史吸收进账本（0 态+默认隔离）
  help                      本帮助

默认账本：当前目录 repo.tritj（-f 可指定）。"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("trit-cli-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn s(v: impl Into<String>) -> String {
        v.into()
    }

    #[test]
    fn full_git_style_workflow() {
        let dir = tmpdir("wf");
        let ledger = dir.join(DEFAULT_LEDGER);

        // init → branch → commit(0 默认) → commit(+1) → status → gate → checkout
        run(&[s("init"), s(dir.to_string_lossy().to_string())]).unwrap();
        assert!(ledger.is_file(), "init 应产生账本");

        run(&[
            s("branch"),
            s("feat-login"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();
        run(&[
            s("commit"),
            s("-m"),
            s("登录模块完成"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();
        run(&[
            s("commit"),
            s("-m"),
            s("回归通过"),
            s("-v"),
            s("+1"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();

        // status 与 gate 不炸且输出非空（走 std::io 捕获太重，只验证 Ok）
        run(&[
            s("status"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();
        run(&[s("gate"), s("-f"), s(ledger.to_string_lossy().to_string())]).unwrap();
        run(&[
            s("suspects"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();
        run(&[
            s("checkout"),
            s("1"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();
        run(&[
            s("log"),
            s("-n"),
            s("3"),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();

        // clone：副本内容一致
        let dst = dir.join("clone.tritj");
        run(&[
            s("clone"),
            s(ledger.to_string_lossy().to_string()),
            s(dst.to_string_lossy().to_string()),
        ])
        .unwrap();
        assert_eq!(
            fs::read_to_string(&ledger).unwrap(),
            fs::read_to_string(&dst).unwrap(),
            "克隆应逐字节一致"
        );

        // merge：克隆侧追加再合回（δ-CRDT 无冲突）
        run(&[
            s("commit"),
            s("-m"),
            s("克隆侧增量"),
            s("-v"),
            s("+1"),
            s("-f"),
            s(dst.to_string_lossy().to_string()),
        ])
        .unwrap();
        run(&[
            s("merge"),
            s(dst.to_string_lossy().to_string()),
            s("-f"),
            s(ledger.to_string_lossy().to_string()),
        ])
        .unwrap();

        // 合并后主账本应含克隆侧增量（Record 不增节点数，追加进 #1 的历史）
        let j = Journal::open_as(&ledger, 0).unwrap();
        let n1 = j.dag().node(NodeId(1));
        assert!(
            n1.history.iter().any(|e| e.note.contains("克隆侧增量")),
            "δ-CRDT 合并应把克隆侧裁决带进来：{:?}",
            n1.history
        );
        drop(j.into_dag().unwrap());

        // sync-git：非 git 目录在 spawn 之前被拒（错误路径，零进程创建）
        let missing = dir.join("not-a-repo");
        assert!(matches!(
            run(&[s("sync-git"), s(missing.to_string_lossy().to_string())]),
            Err(CliError::Runtime(_))
        ));

        // 用法错误路径
        assert!(matches!(
            run(&[s("commit"), s("-m")]),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(run(&[s("nope")]), Err(CliError::Usage(_))));

        fs::remove_dir_all(&dir).unwrap();
    }
}
