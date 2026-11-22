//! `trace sync`：把轨迹事件物化进 SQLite（与轨迹 WAL 解耦）。

use std::path::Path;

use serde_json::{Value, json};
use trit_orch::Error;

use super::TraceInput;

#[cfg(feature = "tritdb-sql")]
pub(super) fn sync_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace sync 需要 path".into(),
    })?;
    let sqlite = Path::new(path).with_extension("sqlite");
    let report = trit_orch::tritdb_backend::sqlite_materialize::sync(Path::new(path), &sqlite)?;
    Ok(json!({
        "ok": true,
        "backend": "tritdb",
        "inserted": report.inserted,
        "watermark": report.watermark,
    }))
}

#[cfg(not(feature = "tritdb-sql"))]
pub(super) fn sync_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace sync 需要编译 feature tritdb-sql".into(),
    })
}
