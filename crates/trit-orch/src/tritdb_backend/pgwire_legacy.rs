//! 旧路径：没有 SQLite 物化库时，直接读轨迹 WAL（会持有 WAL 排他锁）。

use std::net::TcpListener;
use std::path::Path;

use crate::Error;

use crate::tritdb_backend::sql::sync_trace_table;

use super::options;

pub(super) fn serve(db: &Path, listen: &str) -> Result<(), Error> {
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
