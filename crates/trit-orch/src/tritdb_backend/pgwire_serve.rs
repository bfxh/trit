//! pgwire 服务：优先读 SQLite 物化库（与轨迹 WAL 解耦），否则回退直读轨迹 WAL。

use std::path::Path;

use crate::Error;

#[path = "pgwire_legacy.rs"]
mod legacy;
#[path = "pgwire_options.rs"]
mod options;

/// 在 `listen` 上启动 pgwire；阻塞直到服务退出。
pub fn serve_trace(db: &Path, listen: &str) -> Result<(), Error> {
    let sqlite = db.with_extension("sqlite");
    if sqlite.exists() {
        return crate::tritdb_backend::pgwire_sqlite::serve(&sqlite, listen);
    }
    legacy::serve(db, listen)
}
