//! pgwire 服务：启动前把轨迹事件物化进 `trit_trace`，然后提供查询。
//!
//! 服务进程会持续持有 `tritdb-txn` 的 WAL 排他锁，所以服务期间**不能**再有第二个
//! 进程写同一个 WAL（`trace sync` 也会被拒）。要更新数据：先停服，或用
//! `trace sync` 把事件物化到 `<wal>.sqlite`（SQLite 多进程安全）。

use std::net::TcpListener;
use std::path::Path;

use crate::Error;

use crate::tritdb_backend::sql::sync_trace_table;

/// 在 `listen` 上启动 pgwire；阻塞直到服务退出。
pub fn serve_trace(db: &Path, listen: &str) -> Result<(), Error> {
    sync_trace_table(db)?;
    let listener = TcpListener::bind(listen).map_err(|error| io(listen, error))?;
    tritdb_pgwire::serve(db.to_path_buf(), listener)
        .map_err(|error| io(&db.display().to_string(), error))
}

fn io(path: &str, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.to_string(),
        message: error.to_string(),
    }
}
