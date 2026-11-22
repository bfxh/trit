//! 把轨迹事件增量物化成 trit-db SQL 表，供查询与 pgwire 远程查询。
//!
//! 表名固定 `trit_trace`；每次同步只插入 `chain_seq` 大于表中该 trace 最大值的
//! 新事件，不再每次 `DELETE` 重建。物化细节见 [`sync`]。

#[path = "sql_sync.rs"]
mod sync;

use std::path::Path;

use serde::Serialize;
use tritdb_sql::{QueryResult, SqlEngine};

use crate::Error;

use super::TritDbTraceStore;

#[derive(Debug, Clone, Serialize)]
pub struct SqlResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 在 `db` 上执行一条 SQL；先把当前轨迹事件增量同步到 `trit_trace`。
pub fn query_trace_events(db: &Path, sql: &str) -> Result<Vec<SqlResult>, Error> {
    let events = load_events(db)?;
    let mut engine = open_engine(db)?;
    sync::sync_events(&mut engine, &events)?;
    let results = engine
        .execute_script(sql)
        .map_err(|error| self::error(db, error))?;
    Ok(results.into_iter().map(convert).collect())
}

/// 只做增量物化，不执行查询；供 pgwire 启动前调用。
pub fn sync_trace_table(db: &Path) -> Result<(), Error> {
    let events = load_events(db)?;
    let mut engine = open_engine(db)?;
    sync::sync_events(&mut engine, &events)
}

fn load_events(db: &Path) -> Result<Vec<crate::TraceEvent>, Error> {
    let store = TritDbTraceStore::open(db)?;
    store.load()
}

fn open_engine(db: &Path) -> Result<SqlEngine, Error> {
    SqlEngine::open(db).map_err(|error| self::error(db, error))
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

fn error(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceParse {
        path: path.display().to_string(),
        line: 0,
        message: error.to_string(),
    }
}
