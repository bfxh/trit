//! 把事件流压成「它到底干了什么」：进程树、外联、写盘、可疑模式。
//! 网络面的分类与发现规则在 [`crate::net`]（本模块只负责进程/模块/写盘侧）。
//!
//! 规则只陈述**采到的证据**：比如"起了 cmd.exe 并带 -EncodedCommand"是事实，
//! "它在横向移动"不是——那需要更多证据面，报告里不下这种结论。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::event::Event;

#[derive(Debug, Clone)]
pub struct Finding {
    pub id: &'static str,
    pub severity: &'static str,
    pub detail: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub duration_ms: u64,
    pub processes: usize,
    pub modules: usize,
    pub connections: usize,
    pub writes: usize,
    pub findings: Vec<Finding>,
    pub top_cpu: Vec<(String, u64)>,
    pub notes: Vec<String>,
}

const SHELLS: [&str; 6] = [
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "bash.exe",
    "sh.exe",
    "wsl.exe",
];
const NET_TOOLS: [&str; 6] = [
    "curl.exe", "wget.exe", "nc.exe", "ncat.exe", "ssh.exe", "scp.exe",
];
const EXEC_EXT: [&str; 6] = [".exe", ".dll", ".ps1", ".bat", ".cmd", ".scr"];

pub fn analyze(events: &[Event]) -> Report {
    let mut report = Report::default();
    let mut pids: BTreeSet<u32> = BTreeSet::new();
    let mut module_paths: BTreeSet<String> = BTreeSet::new();
    let mut connections: BTreeSet<(u32, String)> = BTreeSet::new();
    let mut echoes: Vec<String> = Vec::new();
    let mut shells: Vec<String> = Vec::new();
    let mut net_tools: Vec<String> = Vec::new();
    let mut temp_modules: Vec<String> = Vec::new();
    let mut exec_writes: Vec<String> = Vec::new();
    let mut encoded_commands: Vec<String> = Vec::new();
    let mut cpu: BTreeMap<String, u64> = BTreeMap::new();
    let mut first_ts = u64::MAX;
    let mut last_ts = 0u64;

    for event in events {
        let ts = match event {
            Event::Process { ts_ms, .. }
            | Event::Module { ts_ms, .. }
            | Event::Connection { ts_ms, .. }
            | Event::FileWrite { ts_ms, .. }
            | Event::Output { ts_ms, .. }
            | Event::Note { ts_ms, .. } => *ts_ms,
        };
        first_ts = first_ts.min(ts);
        last_ts = last_ts.max(ts);
        match event {
            Event::Process {
                ts_ms: _,
                pid,
                name,
                command_line,
                cpu_ms,
                ..
            } => {
                pids.insert(*pid);
                *cpu.entry(name.clone()).or_default() += *cpu_ms;
                let lower = name.to_ascii_lowercase();
                if SHELLS.contains(&lower.as_str()) {
                    shells.push(format!("{name} {command_line}"));
                }
                if NET_TOOLS.contains(&lower.as_str()) {
                    net_tools.push(format!("{name} {command_line}"));
                }
                let cmd_lower = command_line.to_ascii_lowercase();
                if cmd_lower.contains("-encodedcommand") || cmd_lower.contains(" -enc ") {
                    encoded_commands.push(format!("{name} {command_line}"));
                }
            }
            Event::Module { path, .. } => {
                module_paths.insert(path.clone());
                let lower = path.to_ascii_lowercase();
                if lower.contains("\\temp\\")
                    || lower.contains("/tmp/")
                    || lower.contains("\\downloads\\")
                {
                    temp_modules.push(path.clone());
                }
            }
            Event::Connection { pid, remote, .. } => {
                // 连接只计数；网络面发现（外联/监听/扇出/连接失败）在 net.rs 统一判定。
                connections.insert((*pid, remote.clone()));
            }
            Event::FileWrite { path, bytes, .. } => {
                report.writes += 1;
                let lower = path.to_ascii_lowercase();
                if EXEC_EXT.iter().any(|ext| lower.ends_with(ext)) {
                    exec_writes.push(format!("{path} ({bytes} B)"));
                }
            }
            Event::Output { line, .. } => {
                if echoes.len() < 20 {
                    echoes.push(line.clone());
                }
            }
            Event::Note { message, .. } => report.notes.push(message.clone()),
        }
    }

    report.processes = pids.len();
    report.modules = module_paths.len();
    report.connections = connections.len();
    if first_ts <= last_ts {
        report.duration_ms = last_ts - first_ts;
    }

    let mut top: Vec<(String, u64)> = cpu.into_iter().filter(|(_, ms)| *ms > 0).collect();
    top.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    report.top_cpu = top.into_iter().take(5).collect();

    let mut push = |id, severity, detail: String, evidence: Vec<String>| {
        report.findings.push(Finding {
            id,
            severity,
            detail,
            evidence: evidence.into_iter().take(8).collect(),
        });
    };
    if !shells.is_empty() {
        push(
            "spawns-shell",
            "medium",
            format!("起了 {} 个 shell", shells.len()),
            shells,
        );
    }
    if !net_tools.is_empty() {
        push(
            "spawns-network-tool",
            "high",
            format!("起了 {} 个网络工具", net_tools.len()),
            net_tools,
        );
    }
    if !encoded_commands.is_empty() {
        push(
            "encoded-command-line",
            "high",
            "命令行里出现 -EncodedCommand（常见于隐藏载荷）".to_string(),
            encoded_commands,
        );
    }
    if !temp_modules.is_empty() {
        push(
            "loads-module-from-temp",
            "medium",
            format!("从 Temp/Downloads 加载了 {} 个模块", temp_modules.len()),
            temp_modules,
        );
    }
    if !exec_writes.is_empty() {
        push(
            "writes-executable",
            "high",
            format!("写入了 {} 个可执行/脚本文件", exec_writes.len()),
            exec_writes,
        );
    }
    if report.processes > 50 {
        push(
            "many-processes",
            "info",
            format!("跟踪到 {} 个进程", report.processes),
            Vec::new(),
        );
    }
    // 网络面：地址分类 + 外联/监听/扇出/连接失败（见 net 模块的口径与边界）。
    report.findings.extend(crate::net::network_findings(events));
    report
}

pub fn to_json(report: &Report) -> Value {
    json!({
        "tool": "trit-procscan",
        "mode": "analyze",
        "duration_ms": report.duration_ms,
        "processes": report.processes,
        "modules": report.modules,
        "connections": report.connections,
        "file_writes": report.writes,
        "top_cpu": report.top_cpu.iter().map(|(name, ms)| json!({"process": name, "cpu_ms": ms})).collect::<Vec<_>>(),
        "findings": report.findings.iter().map(|finding| json!({
            "id": finding.id,
            "severity": finding.severity,
            "detail": finding.detail,
            "evidence": finding.evidence,
        })).collect::<Vec<_>>(),
        "notes": report.notes,
        "coverage": "进程树/模块/TCP/目录快照；文件读取、注册表、DNS 未采集（非管理员，内核 ETW 不可用）",
    })
}

pub fn has_findings(report: &Report) -> bool {
    report
        .findings
        .iter()
        .any(|finding| finding.severity == "high" || finding.severity == "medium")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, cmd: &str, cpu_ms: u64) -> Event {
        Event::Process {
            ts_ms: 1000 + pid as u64,
            pid,
            ppid: 1,
            name: name.to_string(),
            command_line: cmd.to_string(),
            working_set: 0,
            cpu_ms,
        }
    }

    #[test]
    fn flags_shell_network_and_encoded_commands() {
        let events = vec![
            process(1, "app.exe", "app.exe", 10),
            process(2, "cmd.exe", "cmd.exe /c whoami", 5),
            process(3, "curl.exe", "curl http://x", 5),
            process(
                4,
                "powershell.exe",
                "powershell -EncodedCommand SQBFAFgA",
                5,
            ),
        ];
        let report = analyze(&events);
        let ids: Vec<&str> = report.findings.iter().map(|finding| finding.id).collect();
        for expected in [
            "spawns-shell",
            "spawns-network-tool",
            "encoded-command-line",
        ] {
            assert!(ids.contains(&expected), "{ids:?}");
        }
        assert!(has_findings(&report));
        assert_eq!(report.processes, 4);
    }

    #[test]
    fn loopback_connections_are_not_flagged_but_public_ones_are() {
        let events = vec![
            Event::Connection {
                ts_ms: 1,
                pid: 1,
                remote: "127.0.0.1:3425".into(),
                state: "Established".into(),
            },
            Event::Connection {
                ts_ms: 2,
                pid: 1,
                remote: "10.1.2.3:443".into(),
                state: "Established".into(),
            },
            Event::Connection {
                ts_ms: 3,
                pid: 1,
                remote: "93.184.216.34:443".into(),
                state: "Established".into(),
            },
        ];
        let report = analyze(&events);
        let outbound = report
            .findings
            .iter()
            .find(|finding| finding.id == "outbound-connection")
            .expect("必须报外联");
        assert_eq!(outbound.evidence.len(), 1, "{outbound:?}");
        assert!(outbound.evidence[0].contains("93.184.216.34"));
    }

    #[test]
    fn temp_modules_and_executable_writes_are_flagged() {
        let events = vec![
            Event::Module {
                ts_ms: 1,
                pid: 9,
                process: "x.exe".into(),
                path: "C:\\Users\\u\\AppData\\Local\\Temp\\a.dll".into(),
                exports: None,
            },
            Event::FileWrite {
                ts_ms: 2,
                path: "out\\dropper.exe".into(),
                bytes: 12,
            },
        ];
        let report = analyze(&events);
        let ids: Vec<&str> = report.findings.iter().map(|finding| finding.id).collect();
        assert!(ids.contains(&"loads-module-from-temp"), "{ids:?}");
        assert!(ids.contains(&"writes-executable"), "{ids:?}");
        assert_eq!(report.writes, 1);
    }
}
