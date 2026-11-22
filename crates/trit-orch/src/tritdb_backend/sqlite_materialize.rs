//! 把轨迹事件物化进 **SQLite**（`trit_trace` 表）。
//!
//! 与轨迹 WAL 解耦：读轨迹只短暂持 WAL 锁，写物化表走 SQLite 自己的锁，
//! 因此 `trace sync` 可以与 `trace serve`、以及轨迹写入方并发运行。

use std::path::Path;

use rusqlite::{Connection, params};

use crate::Error;

use super::TritDbTraceStore;

const CREATE: &str = "CREATE TABLE IF NOT EXISTS trit_trace (
    trace_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    chain_seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    actor TEXT NOT NULL,
    observed_at INTEGER NOT NULL,
    recorded_at INTEGER NOT NULL,
    event_time INTEGER,
    payload_hash TEXT NOT NULL,
    payload TEXT,
    PRIMARY KEY (trace_id, chain_seq)
)";

/// 一次同步的结果。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SyncReport {
    /// 本次新写入物化表的行数。
    pub inserted: usize,
    /// 物化表里已有的最大写入序（按 trace 汇总）。
    pub watermark: u64,
}

/// 读取轨迹 WAL 并增量写进 SQLite。轨迹 WAL 只在读取期间持锁。
pub fn sync(db: &Path, sqlite: &Path) -> Result<SyncReport, Error> {
    let events = {
        let store = TritDbTraceStore::open(db)?;
        store.load()?
    };
    let mut conn = Connection::open(sqlite).map_err(|error| io(sqlite, error))?;
    conn.execute_batch(CREATE)
        .map_err(|error| io(sqlite, error))?;
    let watermark: i64 = conn
        .query_row(
            "SELECT COALESCE(max(chain_seq), 0) FROM trit_trace",
            [],
            |row| row.get(0),
        )
        .map_err(|error| io(sqlite, error))?;

    // SQLite 只有 i64；chain_seq 在轨迹里是 u64，物化时按位存 i64。
    let pending: Vec<_> = events
        .into_iter()
        .filter(|event| event.chain_seq as i64 > watermark)
        .collect();
    if pending.is_empty() {
        return Ok(SyncReport {
            inserted: 0,
            watermark: watermark as u64,
        });
    }

    let tx = conn.transaction().map_err(|error| io(sqlite, error))?;
    {
        let mut insert = tx
            .prepare(
                "INSERT OR REPLACE INTO trit_trace
                 (trace_id, seq, chain_seq, kind, actor, observed_at, recorded_at,
                  event_time, payload_hash, payload)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )
            .map_err(|error| io(sqlite, error))?;
        for event in &pending {
            let payload = event
                .payload
                .as_ref()
                .map(|payload| serde_json::to_string(payload).expect("payload"));
            insert
                .execute(params![
                    &event.trace_id,
                    event.seq as i64,
                    event.chain_seq as i64,
                    event.kind.as_str(),
                    format!("{:?}", event.actor),
                    event.observed_at as i64,
                    event.recorded_at as i64,
                    event.event_time.map(|value| value as i64),
                    &event.payload_hash,
                    payload,
                ])
                .map_err(|error| io(sqlite, error))?;
        }
    }
    tx.commit().map_err(|error| io(sqlite, error))?;
    let watermark: i64 = conn
        .query_row(
            "SELECT COALESCE(max(chain_seq), 0) FROM trit_trace",
            [],
            |row| row.get(0),
        )
        .map_err(|error| io(sqlite, error))?;
    Ok(SyncReport {
        inserted: pending.len(),
        watermark: watermark as u64,
    })
}

/// 物化表当前水位。
pub fn watermark(sqlite: &Path) -> Result<u64, Error> {
    let conn = Connection::open(sqlite).map_err(|error| io(sqlite, error))?;
    let value: i64 = conn
        .query_row(
            "SELECT COALESCE(max(chain_seq), 0) FROM trit_trace",
            [],
            |row| row.get(0),
        )
        .map_err(|error| io(sqlite, error))?;
    Ok(value as u64)
}

fn io(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
