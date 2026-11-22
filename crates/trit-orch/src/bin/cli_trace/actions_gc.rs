//! `trace compact`（CAS 链压成单层）与 `trace gc`（物化表按条数保留）。

use std::path::Path;

use serde_json::{Value, json};
use trit_orch::Error;

use super::TraceInput;

fn cas_key() -> Option<Vec<u8>> {
    std::env::var("TRIT_TRACE_CAS_KEY")
        .ok()
        .map(String::into_bytes)
}

#[cfg(feature = "tritdb-cas")]
pub(super) fn compact_action(request: &TraceInput) -> Result<Value, Error> {
    let cas_root = request
        .cas_root
        .as_deref()
        .ok_or_else(|| Error::InputParse {
            message: "trace compact 需要 cas_root".into(),
        })?;
    let key = cas_key();
    let info = trit_orch::tritdb_backend::cas::compact(
        Path::new(cas_root),
        request.head.as_deref(),
        key.as_deref(),
    )?;
    Ok(json!({
        "ok": true,
        "backend": "tritdb",
        "head": info.head,
        "previous": info.previous,
        "layers": info.layers,
    }))
}

#[cfg(not(feature = "tritdb-cas"))]
pub(super) fn compact_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace compact 需要编译 feature tritdb-cas".into(),
    })
}

#[cfg(feature = "tritdb-sql")]
pub(super) fn gc_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace gc 需要 path".into(),
    })?;
    let sqlite = Path::new(path).with_extension("sqlite");
    let keep_last = super::arg_value("--trace-keep-last")
        .and_then(|text| text.parse::<usize>().ok())
        .unwrap_or(10_000);
    let report = trit_orch::tritdb_backend::sqlite_gc::gc(&sqlite, keep_last)?;
    Ok(json!({
        "ok": true,
        "sqlite": sqlite.display().to_string(),
        "total_before": report.total_before,
        "deleted": report.deleted,
        "total_after": report.total_after,
        "keep_last": report.keep_last,
    }))
}

#[cfg(not(feature = "tritdb-sql"))]
pub(super) fn gc_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace gc 需要编译 feature tritdb-sql".into(),
    })
}
