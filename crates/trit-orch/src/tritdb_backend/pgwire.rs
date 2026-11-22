//! 用 tritdb-pgwire 提供轨迹表的远程 PostgreSQL 查询。
//!
//! 启动前先把事件增量物化到 `trit_trace`；服务期间新事件不会自动同步，需要
//! 重新启动或先跑一次同步。

use std::net::TcpListener;
use std::path::Path;

use crate::Error;

use super::sql::sync_trace_table;

/// 在 `listen` 上启动 pgwire；阻塞直到服务退出。
pub fn serve_trace(db: &Path, listen: &str) -> Result<(), Error> {
    sync_trace_table(db)?;
    let listener = TcpListener::bind(listen).map_err(|error| Error::TraceIo {
        path: listen.to_string(),
        message: error.to_string(),
    })?;
    tritdb_pgwire::serve(db.to_path_buf(), listener).map_err(|error| Error::TraceIo {
        path: db.display().to_string(),
        message: error.to_string(),
    })
}
