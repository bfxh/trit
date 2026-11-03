//! 现场采集（N3）：procscan 事件流 → 模块路径 → 逆向台账。
//!
//! 与 `trit-procscan` 的关系是**管道**而不是依赖：procscan 在现场输出 JSONL
//! 事件（`{"type":"module","path":…}`），本模块吃这条流，抽取模块路径，
//! 逐个跑 `analyze` 产出台账行，最后给一条**采集汇总**——路线图 N3 的判据
//! （一次采集产出 ≥ N 条记录、耗时与文件大小线性）直接从汇总行复算。
//!
//! 纪律（每条可测）：
//! - 事件流是**外部输入**：坏 JSON 行计数上报不静默丢（与 pcap 层同一纪律）；
//! - 非 `module` 事件（process/connection/file-write/…）跳过并计数——procscan
//!   的流是全事件的，这里只消费模块面；
//! - 同一路径**去重保序**：同一模块被 N 个进程加载只分析一次（`deduped` 计数）；
//! - 单个文件读不到/分析失败**不中断采集**：计数（`unreadable`）后继续——
//!   现场里模块正被写入是常态；
//! - 汇总行带 `elapsed_ms` 与总字节数，线性性由操作者用两次采集对比复算，
//!   本模块只保证**如实报告**。
//!
//! 边界（🔴）：路径来自现场事件流，本模块**只读这些文件**，不追链接、
//! 不解析协议路径（`\\?\` 前缀等交由调用方规范化）；读取失败的原因以
//! OS 错误文本进汇总，不猜。

use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;

use crate::ledger;
use crate::triage::Triage;

/// 一条采集汇总（台账的最后一行）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CollectSummary {
    /// 记录类型（固定 `collect-summary`；`String` 是为了随行走 Deserialize）。
    pub kind: String,
    /// 事件流里 `module` 事件的条数（含重复）。
    pub module_events: usize,
    /// 去重后的唯一路径数。
    pub modules: usize,
    /// 同路径重复出现被跳过的次数。
    pub deduped: usize,
    /// 成功分析的模块数。
    pub analyzed: usize,
    /// 读取失败的模块数（原因进 `errors`）。
    pub unreadable: usize,
    /// 非 `module` 事件数。
    pub non_module_events: usize,
    /// 坏 JSON 行数。
    pub bad_lines: usize,
    /// 成功分析的模块总字节数。
    pub bytes: u64,
    /// 采集耗时（毫秒，含分析）。
    pub elapsed_ms: u64,
    /// 读取失败的原因（路径 → OS 错误文本，≤8 条）。
    pub errors: Vec<String>,
}

/// 事件流里我们关心的最小字段。
#[derive(Deserialize)]
struct ModuleEvent<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(default)]
    path: String,
}

/// 从 procscan JSONL 事件流抽取模块路径：去重、保序。
/// 返回（路径序列, 模块事件数, 去重跳过数, 非 module 事件数, 坏行数）。
pub fn module_paths(events_text: &str) -> (Vec<PathBuf>, usize, usize, usize, usize) {
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let (mut module_events, mut deduped, mut non_module, mut bad_lines) = (0usize, 0, 0, 0);
    for line in events_text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let event: ModuleEvent = match serde_json::from_str(line) {
            Ok(event) => event,
            Err(_) => {
                bad_lines += 1;
                continue;
            }
        };
        if event.kind != "module" {
            non_module += 1;
            continue;
        }
        module_events += 1;
        if event.path.is_empty() {
            bad_lines += 1;
            continue;
        }
        let path = PathBuf::from(&event.path);
        if seen.insert(path.clone()) {
            paths.push(path);
        } else {
            deduped += 1;
        }
    }
    (paths, module_events, deduped, non_module, bad_lines)
}

/// 执行一次采集：逐个分析模块并产出台账行 + 汇总行。
///
/// `read` 由调用方注入（CLI 传 [`std::fs::read`]，测试传内存闭包）；
/// 每个成功模块产出 `ledger::jsonl` 的行（顺序与路径一致），最后追加
/// 一条 `collect-summary` 行。单文件失败不中断——计进 `unreadable`。
pub fn collect(
    events_text: &str,
    read: impl Fn(&Path) -> io::Result<Vec<u8>>,
) -> (Vec<String>, CollectSummary) {
    let started = Instant::now();
    let (paths, module_events, deduped, non_module, bad_lines) = module_paths(events_text);
    let mut rows = Vec::new();
    let mut summary = CollectSummary {
        kind: "collect-summary".to_string(),
        module_events,
        modules: paths.len(),
        deduped,
        analyzed: 0,
        unreadable: 0,
        non_module_events: non_module,
        bad_lines,
        bytes: 0,
        elapsed_ms: 0,
        errors: Vec::new(),
    };
    for path in &paths {
        match read(path) {
            Ok(bytes) => {
                let triage: Triage = crate::analyze(&bytes);
                summary.bytes += triage.size;
                summary.analyzed += 1;
                rows.extend(ledger::jsonl(&triage));
            }
            Err(error) => {
                summary.unreadable += 1;
                if summary.errors.len() < 8 {
                    summary.errors.push(format!("{}: {error}", path.display()));
                }
            }
        }
    }
    summary.elapsed_ms = started.elapsed().as_millis() as u64;
    rows.push(serde_json::to_string(&summary).expect("采集汇总序列化不会失败"));
    (rows, summary)
}
