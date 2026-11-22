#[cfg(any(
    feature = "tritdb-sql",
    feature = "tritdb-cas",
    feature = "tritdb-pgwire"
))]
use std::path::Path;

use serde_json::Value;
use trit_orch::Error;

use super::TraceInput;

#[cfg(feature = "tritdb-sql")]
pub(super) fn sql_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace sql 需要 path".into(),
    })?;
    let sql = request.sql.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace sql 需要 sql".into(),
    })?;
    let results = trit_orch::tritdb_backend::sql::query_trace_events(Path::new(path), sql)?;
    Ok(serde_json::json!({
        "ok": true,
        "backend": "tritdb",
        "results": results,
    }))
}

#[cfg(not(feature = "tritdb-sql"))]
pub(super) fn sql_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace sql 需要编译 feature tritdb-sql".into(),
    })
}

#[cfg(feature = "tritdb-cas")]
pub(super) fn snapshot_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace snapshot 需要 path".into(),
    })?;
    let cas_root = request
        .cas_root
        .as_deref()
        .ok_or_else(|| Error::InputParse {
            message: "trace snapshot 需要 cas_root".into(),
        })?;
    let info = trit_orch::tritdb_backend::cas::snapshot_wal_with_key(
        Path::new(path),
        Path::new(cas_root),
        cas_key().as_deref(),
    )?;
    Ok(serde_json::json!({
        "ok": true,
        "backend": "tritdb",
        "head": info.head,
        "parent": info.parent,
        "anchor": info.anchor,
    }))
}

#[cfg(not(feature = "tritdb-cas"))]
pub(super) fn snapshot_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace snapshot 需要编译 feature tritdb-cas".into(),
    })
}

#[cfg(feature = "tritdb-cas")]
pub(super) fn restore_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace restore 需要 path".into(),
    })?;
    let cas_root = request
        .cas_root
        .as_deref()
        .ok_or_else(|| Error::InputParse {
            message: "trace restore 需要 cas_root".into(),
        })?;
    let restored = trit_orch::tritdb_backend::cas::restore_wal_from(
        Path::new(path),
        Path::new(cas_root),
        request.head.as_deref(),
        cas_key().as_deref(),
    )?;
    Ok(serde_json::json!({
        "ok": true,
        "backend": "tritdb",
        "restored": restored,
    }))
}

#[cfg(not(feature = "tritdb-cas"))]
pub(super) fn restore_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace restore 需要编译 feature tritdb-cas".into(),
    })
}

#[cfg(feature = "tritdb-cas")]
fn cas_key() -> Option<Vec<u8>> {
    std::env::var("TRIT_TRACE_CAS_KEY")
        .ok()
        .map(String::into_bytes)
}

#[cfg(feature = "tritdb-pgwire")]
pub(super) fn serve_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace serve 需要 path".into(),
    })?;
    let listen = request.listen.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace serve 需要 listen".into(),
    })?;
    trit_orch::tritdb_backend::pgwire::serve_trace(Path::new(path), listen)?;
    Ok(serde_json::json!({
        "ok": true,
        "backend": "tritdb",
        "served": listen,
    }))
}

#[cfg(not(feature = "tritdb-pgwire"))]
pub(super) fn serve_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace serve 需要编译 feature tritdb-pgwire".into(),
    })
}
