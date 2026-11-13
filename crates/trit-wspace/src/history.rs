//! 变更历史：从审计日志汇总「做了什么改良」。
//!
//! 审计日志没有墙钟时间，只有严格的 `seq` 顺序；这里按顺序给出时间线，
//! 统计应用/回滚/跳过与每类记录名的次数。

use std::collections::BTreeMap;

use crate::audit::AuditLog;

/// 一条历史事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryEvent {
    /// 序号。
    pub seq: usize,
    /// 记录名。
    pub name: String,
    /// 是否应用。
    pub applied: bool,
    /// 是否回滚。
    pub rolled_back: bool,
    /// 补丁里的编辑数。
    pub edits: usize,
}

/// 历史摘要。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistorySummary {
    /// 记录条数。
    pub entries: usize,
    /// 实际应用条数。
    pub applied: usize,
    /// 回滚条数。
    pub rolled_back: usize,
    /// 未应用条数。
    pub skipped: usize,
    /// 编辑总数。
    pub edits: usize,
    /// 按记录名分组。
    pub by_name: Vec<(String, usize)>,
    /// 按 `seq` 排列的事件。
    pub events: Vec<HistoryEvent>,
}

/// 汇总审计日志。
#[must_use]
pub fn summary(log: &AuditLog) -> HistorySummary {
    let mut by_name: BTreeMap<&str, usize> = BTreeMap::new();
    let mut applied = 0usize;
    let mut rolled_back = 0usize;
    let mut skipped = 0usize;
    let mut edits = 0usize;
    let mut events = Vec::with_capacity(log.len());
    for entry in log.entries() {
        if entry.applied {
            applied += 1;
        } else {
            skipped += 1;
        }
        if entry.rolled_back {
            rolled_back += 1;
        }
        let count = entry.patch.len();
        edits += count;
        *by_name.entry(entry.name.as_str()).or_default() += 1;
        events.push(HistoryEvent {
            seq: entry.seq,
            name: entry.name.clone(),
            applied: entry.applied,
            rolled_back: entry.rolled_back,
            edits: count,
        });
    }
    HistorySummary {
        entries: log.len(),
        applied,
        rolled_back,
        skipped,
        edits,
        by_name: by_name
            .into_iter()
            .map(|(name, count)| (name.to_string(), count))
            .collect(),
        events,
    }
}
