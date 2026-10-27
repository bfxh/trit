//! 动态证据的追溯与追踪：对采集到的 JSONL 事件流做离线重放分析。
//!
//! - **追溯（traceback）**：给定 pid，沿 `ppid` 链回溯到采样里能看到的根，
//!   每一环带首次/末次出现时间与命令行。父进程不在采样里时**显式说明**（不猜）；
//!   出现环时记录并停止——采样伪影或刻意伪装，追到哪算哪。
//! - **追踪（follow）**：正向收齐该 pid 的全部后代（多级子树），按首次出现排序。
//! - **证据绑定**：模块 / TCP 连接事件按 pid 绑到链上成员；目录写入没有 pid，
//!   单列全程时间线；采集器说明（Note）原样带出。
//!
//! 纯离线、确定性：同一份 JSONL 永远给出同一份追溯报告（可当复算门）。
//! 边界：只能追采样里出现过的东西——「没采到」在 notes 里写明，绝不补全。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::event::Event;

/// 链上/树上一员的画像（首个采样给出名字与命令行；后续采样只更新时间与次数）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStep {
    /// 进程 id。
    pub pid: u32,
    /// 父进程 id（取首次采样）。
    pub ppid: u32,
    /// 进程名。
    pub name: String,
    /// 命令行。
    pub command_line: String,
    /// 首次被采样到的时刻（ms）。
    pub first_seen_ms: u64,
    /// 末次被采样到的时刻（ms）。
    pub last_seen_ms: u64,
    /// 被采样到的次数。
    pub samples: usize,
}

/// 一次追溯/追踪的报告。
#[derive(Debug, Clone, PartialEq)]
pub struct TraceReport {
    /// 追溯目标。
    pub target: u32,
    /// 采样里能看到的祖先链根（链条断在采样边界时 = target 自己）。
    pub root: u32,
    /// 祖先链（root → target 方向）。
    pub ancestry: Vec<TraceStep>,
    /// 后代子树（按首次出现时间排序，含间接后代）。
    pub descendants: Vec<TraceStep>,
    /// 绑定到链上成员的证据事件（模块 / 连接，按时间排序）。
    pub evidence: Vec<Value>,
    /// 监视目录的写入（没有 pid，列全程时间线）。
    pub file_writes: Vec<Value>,
    /// 采样器说明 + 追溯中的显式缺口说明（不猜原则的出口）。
    pub notes: Vec<String>,
    /// 参与本次追溯的事件总数。
    pub events_scanned: usize,
}

/// pid → 画像。重复采样合并：首个样本定名字/命令行/ppid，
/// 末次时间取最大，次数累加。
fn profiles(events: &[Event]) -> BTreeMap<u32, TraceStep> {
    let mut out: BTreeMap<u32, TraceStep> = BTreeMap::new();
    for event in events {
        if let Event::Process {
            ts_ms,
            pid,
            ppid,
            name,
            command_line,
            ..
        } = event
        {
            let entry = out.entry(*pid).or_insert_with(|| TraceStep {
                pid: *pid,
                ppid: *ppid,
                name: name.clone(),
                command_line: command_line.clone(),
                first_seen_ms: *ts_ms,
                last_seen_ms: *ts_ms,
                samples: 0,
            });
            entry.last_seen_ms = entry.last_seen_ms.max(*ts_ms);
            entry.samples += 1;
        }
    }
    out
}

/// 追溯 + 追踪一份事件流。`pid` 不在采样里时返回带说明的空报告。
pub fn trace(events: &[Event], pid: u32) -> TraceReport {
    let profiles = profiles(events);
    let mut notes = Vec::new();

    // —— 追溯：沿 ppid 链向根走。每条路径都推进 current，环用 visited 拦住。——
    let mut chain: Vec<u32> = Vec::new();
    let mut visited: BTreeSet<u32> = BTreeSet::new();
    let mut current = pid;
    loop {
        if !visited.insert(current) {
            notes.push(format!(
                "ppid 链出现环（pid {current} 重复出现），追溯在此停止——采样伪影或刻意伪装，不猜。"
            ));
            break;
        }
        if current == 0 {
            // Windows 语义：ppid=0 = 没有存活父进程（或已是根），是正常链尾。
            notes.push(format!(
                "ppid=0：pid {pid} 的祖先链在采样边界内走完（无存活父进程）。"
            ));
            break;
        }
        let Some(step) = profiles.get(&current) else {
            if chain.is_empty() {
                notes.push(format!(
                    "pid {pid} 未出现在本次采样的进程事件里（间隔漏采或目标已退出），追溯为空。"
                ));
            } else {
                notes.push(format!(
                    "父进程 {current} 未在采样中出现（采样间隔漏采或不在目标子树），链条到此为止，不猜。"
                ));
            }
            break;
        };
        chain.push(current);
        current = step.ppid;
    }
    chain.reverse();
    let ancestry: Vec<TraceStep> = chain.iter().map(|p| profiles[p].clone()).collect();
    let root = chain.first().copied().unwrap_or(pid);

    // —— 追踪：正向收齐后代子树。 ——
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (pid, step) in &profiles {
        children.entry(step.ppid).or_default().push(*pid);
    }
    let mut descendants: Vec<TraceStep> = Vec::new();
    let mut seen: BTreeSet<u32> = BTreeSet::from([pid]);
    let mut worklist: Vec<u32> = children.get(&pid).cloned().unwrap_or_default();
    while let Some(current) = worklist.pop() {
        if !seen.insert(current) {
            continue;
        }
        if let Some(step) = profiles.get(&current) {
            descendants.push(step.clone());
            worklist.extend(children.get(&current).cloned().unwrap_or_default());
        }
    }
    descendants.sort_by_key(|step| (step.first_seen_ms, step.pid));

    // —— 证据绑定：链上成员（祖先 + 后代）的模块 / 连接；目录写入全程单列。 ——
    let mut chain_pids: BTreeSet<u32> = chain.iter().copied().collect();
    for step in &descendants {
        chain_pids.insert(step.pid);
    }
    let mut evidence = Vec::new();
    let mut file_writes = Vec::new();
    for event in events {
        match event {
            Event::Module { .. } | Event::Connection { .. } => {
                let pid = match event {
                    Event::Module { pid, .. } | Event::Connection { pid, .. } => *pid,
                    _ => unreachable!(),
                };
                if chain_pids.contains(&pid) {
                    evidence.push(event.to_json());
                }
            }
            Event::FileWrite { .. } => file_writes.push(event.to_json()),
            Event::Note { message, .. } => notes.push(message.clone()),
            Event::Process { .. } | Event::Output { .. } => {}
        }
    }
    evidence.sort_by_key(|value| value["ts_ms"].as_u64().unwrap_or(0));
    file_writes.sort_by_key(|value| value["ts_ms"].as_u64().unwrap_or(0));

    TraceReport {
        target: pid,
        root,
        ancestry,
        descendants,
        evidence,
        file_writes,
        notes,
        events_scanned: events.len(),
    }
}

fn step_json(step: &TraceStep) -> Value {
    json!({
        "pid": step.pid,
        "ppid": step.ppid,
        "name": step.name,
        "command_line": step.command_line,
        "first_seen_ms": step.first_seen_ms,
        "last_seen_ms": step.last_seen_ms,
        "samples": step.samples,
    })
}

/// 报告转 CLI 的 JSON 输出。
pub fn to_json(report: &TraceReport) -> Value {
    json!({
        "target": report.target,
        "root": report.root,
        "events_scanned": report.events_scanned,
        "ancestry": report.ancestry.iter().map(step_json).collect::<Vec<_>>(),
        "descendants": report.descendants.iter().map(step_json).collect::<Vec<_>>(),
        "evidence": report.evidence,
        "file_writes": report.file_writes,
        "notes": report.notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(ts: u64, pid: u32, ppid: u32, name: &str) -> Event {
        Event::Process {
            ts_ms: ts,
            pid,
            ppid,
            name: name.into(),
            command_line: format!("{name} --run"),
            working_set: 1,
            cpu_ms: 0,
        }
    }

    /// 三代链：root(100) → mid(200) → target(300)，外加一个树外旁支(400，ppid=1)。
    fn family() -> Vec<Event> {
        vec![
            proc(10, 100, 1, "root.exe"),
            proc(20, 200, 100, "mid.exe"),
            proc(30, 400, 1, "side.exe"),
            proc(40, 300, 200, "target.exe"),
        ]
    }

    #[test]
    fn ancestry_walks_to_root_in_root_first_order() {
        let report = trace(&family(), 300);
        let pids: Vec<u32> = report.ancestry.iter().map(|s| s.pid).collect();
        assert_eq!(pids, vec![100, 200, 300], "root → target 方向");
        assert_eq!(report.root, 100);
        // 时间画像：首次/末次/次数。
        let target = &report.ancestry[2];
        assert_eq!(target.first_seen_ms, 40);
        assert_eq!(target.last_seen_ms, 40);
        assert_eq!(target.samples, 1);
        assert!(report.descendants.is_empty(), "叶子没有后代");
    }

    #[test]
    fn repeated_samples_merge_into_one_step() {
        let events = vec![proc(10, 300, 200, "a.exe"), proc(50, 300, 200, "a.exe")];
        let report = trace(&events, 300);
        assert_eq!(report.ancestry.len(), 1);
        let step = &report.ancestry[0];
        assert_eq!(step.first_seen_ms, 10);
        assert_eq!(step.last_seen_ms, 50);
        assert_eq!(step.samples, 2);
        // 首个样本的名字保留。
        assert_eq!(step.name, "a.exe");
    }

    #[test]
    fn missing_parent_is_reported_not_guessed() {
        let events = vec![proc(10, 300, 999, "target.exe")];
        let report = trace(&events, 300);
        assert_eq!(report.ancestry.len(), 1);
        assert_eq!(report.root, 300, "链条断在采样边界");
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("999") && n.contains("不猜")),
            "缺口要显式说明：{:?}",
            report.notes
        );
    }

    #[test]
    fn zero_ppid_is_a_normal_chain_end() {
        // Windows：ppid=0 = 无存活父进程（真机采集常见，cmd 一闪而过时父链就是这样）。
        let events = vec![proc(10, 300, 0, "lonely.exe")];
        let report = trace(&events, 300);
        let pids: Vec<u32> = report.ancestry.iter().map(|s| s.pid).collect();
        assert_eq!(pids, vec![300]);
        assert_eq!(report.root, 300);
        assert!(
            report
                .notes
                .iter()
                .any(|n| n.contains("ppid=0") && n.contains("无存活父进程")),
            "{:?}",
            report.notes
        );
        // 不能把它当缺口报「父进程 0 未在采样中出现」。
        assert!(!report.notes.iter().any(|n| n.contains("不猜")));
    }

    #[test]
    fn cycle_is_detected_and_stops() {
        // a ↔ b 互相指认：采样伪影或伪装。追溯必须停，不能死循环。
        let events = vec![proc(10, 100, 200, "a"), proc(20, 200, 100, "b")];
        let report = trace(&events, 100);
        assert!(
            report.notes.iter().any(|n| n.contains("环")),
            "{:?}",
            report.notes
        );
        assert!(report.ancestry.len() <= 2);
    }

    #[test]
    fn unknown_pid_yields_empty_report_with_note() {
        let report = trace(&family(), 777);
        assert!(report.ancestry.is_empty());
        assert!(report.descendants.is_empty());
        assert_eq!(report.root, 777);
        assert!(report.notes.iter().any(|n| n.contains("777")));
    }

    #[test]
    fn descendants_follow_multi_level_tree() {
        let mut events = family();
        events.push(proc(50, 500, 300, "grandchild.exe"));
        let report = trace(&events, 100);
        let pids: Vec<u32> = report.descendants.iter().map(|s| s.pid).collect();
        // 间接后代也要收进（200 与 300 的子树），按首次出现排序；旁支 400 不算。
        assert_eq!(pids, vec![200, 300, 500]);
    }

    #[test]
    fn evidence_binds_to_chain_pids_only() {
        let mut events = family();
        events.push(Event::Module {
            ts_ms: 35,
            pid: 300,
            process: "target.exe".into(),
            path: "C:\\evil.dll".into(),
            exports: Some(3),
        });
        events.push(Event::Connection {
            ts_ms: 36,
            pid: 400, // 旁支：不在 300 的链上
            remote: "1.2.3.4:443".into(),
            state: "Established".into(),
        });
        events.push(Event::FileWrite {
            ts_ms: 37,
            path: "C:\\watch\\out.txt".into(),
            bytes: 10,
        });
        events.push(Event::Output {
            ts_ms: 38,
            stream: "stdout".into(),
            line: "hello".into(),
        });
        events.push(Event::Note {
            ts_ms: 39,
            message: "文件读取未采集".into(),
        });
        let report = trace(&events, 300);
        // 证据只绑链上（300）的模块/连接；旁支 400 的连接不进来。
        assert_eq!(report.evidence.len(), 1, "{:?}", report.evidence);
        assert_eq!(report.evidence[0]["path"], "C:\\evil.dll");
        // 目录写入没有 pid，全程单列。
        assert_eq!(report.file_writes.len(), 1);
        // 采集器说明带出。
        assert!(report.notes.iter().any(|n| n.contains("未采集")));
    }

    #[test]
    fn jsonl_roundtrip_traces_identically() {
        // 离线重放路径：to_json → from_json 追溯结果必须一致（复算门）。
        let events = family();
        let direct = trace(&events, 300);
        let replayed: Vec<Event> = events
            .iter()
            .map(|e| Event::from_json(&e.to_json()).expect("往返还原"))
            .collect();
        assert_eq!(trace(&replayed, 300), direct);
    }

    #[test]
    fn json_report_shape() {
        let report = trace(&family(), 300);
        let value = to_json(&report);
        assert_eq!(value["target"], 300);
        assert_eq!(value["root"], 100);
        assert_eq!(value["ancestry"].as_array().unwrap().len(), 3);
        assert_eq!(value["events_scanned"], 4);
    }
}
