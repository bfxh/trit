//! 物化表 GC：按条数保留最近 N 条，永不删除 watermark 所在行。
//!
//! 为什么保护 watermark：`trace sync` 的增量判据是 `chain_seq > max(chain_seq)`。
//! 只要最大 `chain_seq` 那行还在，删掉更早的行不会被 sync 重新插回来。

use std::path::Path;

use rusqlite::{Connection, params};
use serde::Serialize;

use crate::Error;

#[derive(Debug, Clone, Serialize)]
pub struct GcReport {
    pub total_before: u64,
    pub deleted: u64,
    pub total_after: u64,
    pub keep_last: u64,
}

/// 保留 `chain_seq` 最大的 `keep_last` 行（默认调用方传 10000）。
pub fn gc(sqlite: &Path, keep_last: usize) -> Result<GcReport, Error> {
    let conn = Connection::open(sqlite).map_err(|error| io(sqlite, error))?;
    let total_before: i64 = conn
        .query_row("SELECT count(*) FROM trit_trace", [], |row| row.get(0))
        .map_err(|error| io(sqlite, error))?;
    if total_before as u64 <= keep_last as u64 {
        return Ok(GcReport {
            total_before: total_before as u64,
            deleted: 0,
            total_after: total_before as u64,
            keep_last: keep_last as u64,
        });
    }

    // 第 keep_last 旧的边界：chain_seq 严格小于它就删。
    let cutoff: Option<i64> = conn
        .query_row(
            "SELECT chain_seq FROM trit_trace ORDER BY chain_seq DESC LIMIT 1 OFFSET ?1",
            params![keep_last as i64 - 1],
            |row| row.get(0),
        )
        .ok();
    let Some(cutoff) = cutoff else {
        return Ok(GcReport {
            total_before: total_before as u64,
            deleted: 0,
            total_after: total_before as u64,
            keep_last: keep_last as u64,
        });
    };

    let deleted = conn
        .execute(
            "DELETE FROM trit_trace WHERE chain_seq < ?1",
            params![cutoff],
        )
        .map_err(|error| io(sqlite, error))? as u64;
    let total_after: i64 = conn
        .query_row("SELECT count(*) FROM trit_trace", [], |row| row.get(0))
        .map_err(|error| io(sqlite, error))?;
    Ok(GcReport {
        total_before: total_before as u64,
        deleted,
        total_after: total_after as u64,
        keep_last: keep_last as u64,
    })
}

fn io(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
