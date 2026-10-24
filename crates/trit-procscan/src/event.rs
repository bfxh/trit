//! 事件模型：采集到的每一条证据都是带时间的结构化事件（JSONL）。
//!
//! 采集面的现实约束（2026-10-25 实测）：本机**不是管理员**，内核 ETW 的
//! 进程/文件/镜像提供程序都起不来，所以 v1 只用得到这些证据：
//! 进程树与命令行、各进程加载的模块、TCP 连接、目录快照差异、CPU/内存。
//! 拿不到的（谁读了哪个文件、谁写了注册表）在报告里显式写"未采集"，不猜。

use serde_json::{Value, json};

#[derive(Debug, Clone)]
pub enum Event {
    /// 采样时刻的进程快照（差分后得到"新起了谁"）。
    Process {
        ts_ms: u64,
        pid: u32,
        ppid: u32,
        name: String,
        command_line: String,
        working_set: u64,
        cpu_ms: u64,
    },
    /// 某个进程加载的模块（开始/结束各采一次，差集即"运行期新加载"）。
    Module {
        ts_ms: u64,
        pid: u32,
        process: String,
        path: String,
        exports: Option<u32>,
    },
    /// 该进程持有的 TCP 连接。
    Connection {
        ts_ms: u64,
        pid: u32,
        remote: String,
        state: String,
    },
    /// 监视目录里新增/修改的文件。
    FileWrite {
        ts_ms: u64,
        path: String,
        bytes: u64,
    },
    /// 目标进程自己写到 stdout/stderr 的行（如果以捕获模式启动）。
    Output {
        ts_ms: u64,
        stream: String,
        line: String,
    },
    /// 采集器自己说明"这段时间采不到什么"。
    Note { ts_ms: u64, message: String },
}

impl Event {
    pub fn to_json(&self) -> Value {
        match self {
            Self::Process {
                ts_ms,
                pid,
                ppid,
                name,
                command_line,
                working_set,
                cpu_ms,
            } => json!({
                "type": "process", "ts_ms": ts_ms, "pid": pid, "ppid": ppid,
                "name": name, "command_line": command_line,
                "working_set": working_set, "cpu_ms": cpu_ms,
            }),
            Self::Module {
                ts_ms,
                pid,
                process,
                path,
                exports,
            } => json!({
                "type": "module", "ts_ms": ts_ms, "pid": pid,
                "process": process, "path": path, "exports": exports,
            }),
            Self::Connection {
                ts_ms,
                pid,
                remote,
                state,
            } => json!({
                "type": "connection", "ts_ms": ts_ms, "pid": pid,
                "remote": remote, "state": state,
            }),
            Self::FileWrite { ts_ms, path, bytes } => json!({
                "type": "file-write", "ts_ms": ts_ms, "path": path, "bytes": bytes,
            }),
            Self::Output {
                ts_ms,
                stream,
                line,
            } => json!({
                "type": "output", "ts_ms": ts_ms, "stream": stream, "line": line,
            }),
            Self::Note { ts_ms, message } => json!({
                "type": "note", "ts_ms": ts_ms, "message": message,
            }),
        }
    }
}

impl Event {
    /// 从 JSONL 行还原事件（`analyze` 离线重放用）。
    pub fn from_json(value: &Value) -> Option<Self> {
        let ts_ms = value.get("ts_ms")?.as_u64()?;
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        Some(match value.get("type")?.as_str()? {
            "process" => Self::Process {
                ts_ms,
                pid: value.get("pid")?.as_u64()? as u32,
                ppid: value.get("ppid").and_then(Value::as_u64).unwrap_or(0) as u32,
                name: text("name"),
                command_line: text("command_line"),
                working_set: value
                    .get("working_set")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                cpu_ms: value.get("cpu_ms").and_then(Value::as_u64).unwrap_or(0),
            },
            "module" => Self::Module {
                ts_ms,
                pid: value.get("pid")?.as_u64()? as u32,
                process: text("process"),
                path: text("path"),
                exports: value
                    .get("exports")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
            },
            "connection" => Self::Connection {
                ts_ms,
                pid: value.get("pid")?.as_u64()? as u32,
                remote: text("remote"),
                state: text("state"),
            },
            "file-write" => Self::FileWrite {
                ts_ms,
                path: text("path"),
                bytes: value.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            },
            "output" => Self::Output {
                ts_ms,
                stream: text("stream"),
                line: text("line"),
            },
            "note" => Self::Note {
                ts_ms,
                message: text("message"),
            },
            _ => return None,
        })
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
