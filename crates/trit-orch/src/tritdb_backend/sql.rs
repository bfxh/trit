//! 把轨迹事件物化成 trit-db SQL 表，供 `trit-orch trace --trace-backend tritdb`
//! 的 `sql` action 查询。
//!
//! 表名固定 `trit_trace`；每次查询前重建，事件从同一个 WAL 的 KV 中读取。

use std::path::Path;

use serde::Serialize;
use tritdb_sql::{QueryResult, SqlEngine};

use crate::Error;

use super::TritDbTraceStore;

const TABLE: &str = "trit_trace";

#[derive(Debug, Clone, Serialize)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 在 `db` 上执行一条 SQL；先把当前轨迹事件物化到 `trit_trace`。
pub fn query_trace_events(db: &Path, sql: &str) -> Result<Vec<SqlResult>, Error> {
    let events = {
        let store = TritDbTraceStore::open(db)?;
        store.load()?
    };
    let mut engine = SqlEngine::open(db).map_err(|error| self::error(db, error))?;
    engine
        .execute_script(&format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (\
             trace_id TEXT, seq INT, chain_seq INT, kind TEXT, actor TEXT, \
             observed_at INT, recorded_at INT, event_time INT, payload_hash TEXT, payload TEXT);\
             DELETE FROM {TABLE};"
        ))
        .map_err(|error| self::error(db, error))?;

    for chunk in events.chunks(64) {
        let mut script = format!("INSERT INTO {TABLE} VALUES ");
        for (index, event) in chunk.iter().enumerate() {
            if index > 0 {
                script.push(',');
            }
            let payload = match &event.payload {
                Some(payload) => quote(&serde_json::to_string(payload).expect("payload")),
                None => "NULL".to_string(),
            };
            let event_time = match event.event_time {
                Some(value) => value.to_string(),
                None => "NULL".to_string(),
            };
            script.push_str(&format!(
                "({},{},{},{},{},{},{},{},{},{})",
                quote(&event.trace_id),
                event.seq,
                event.chain_seq,
                quote(event.kind.as_str()),
                quote(&format!("{:?}", event.actor)),
                event.observed_at,
                event.recorded_at,
                event_time,
                quote(&event.payload_hash),
                payload
            ));
        }
        script.push(';');
        engine
            .execute_script(&script)
            .map_err(|error| self::error(db, error))?;
    }

    let results = engine
        .execute_script(sql)
        .map_err(|error| self::error(db, error))?;
    Ok(results.into_iter().map(convert).collect())
}

fn convert(result: QueryResult) -> SqlResult {
    match result {
        QueryResult::Rows { columns, rows } => SqlResult {
            columns,
            rows: rows
                .into_iter()
                .map(|row| row.into_iter().map(|value| value.display()).collect())
                .collect(),
            message: None,
        },
        QueryResult::Message(message) => SqlResult {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some(message),
        },
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn error(db: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceParse {
        path: db.display().to_string(),
        line: 0,
        message: error.to_string(),
    }
}
