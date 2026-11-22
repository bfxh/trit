//! pgwire 服务：启动前物化再查询；服务持有轨迹 WAL 排他锁，期间不能再写同一 WAL。

use std::net::TcpListener;
use std::path::Path;

use crate::Error;

use crate::tritdb_backend::sql::sync_trace_table;

#[path = "pgwire_options.rs"]
mod options;

/// 在 `listen` 上启动 pgwire；阻塞直到服务退出。
pub fn serve_trace(db: &Path, listen: &str) -> Result<(), Error> {
    sync_trace_table(db)?;
    let listener = TcpListener::bind(listen).map_err(|error| io(listen, error))?;
    let options = options::server_options()?;
    tritdb_pgwire::serve_with_options(db.to_path_buf(), listener, options)
        .map_err(|error| io(&db.display().to_string(), error))
}

fn io(path: &str, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.to_string(),
        message: error.to_string(),
    }
}
