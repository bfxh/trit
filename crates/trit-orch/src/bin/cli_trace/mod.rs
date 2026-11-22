#[cfg(any(feature = "tritdb-sql", feature = "tritdb-cas"))]
use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};
use trit_orch::{Error, TraceEvent, TraceEventStore, TraceLog, check_trace};

#[cfg(feature = "tritdb")]
use trit_orch::tritdb_backend::TritDbTraceStore;

#[allow(dead_code)]
#[derive(Deserialize)]
struct TraceInput {
    action: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    events: Vec<TraceEvent>,
    #[serde(default)]
    trace_id: Option<String>,
    #[serde(default)]
    sql: Option<String>,
    #[serde(default)]
    cas_root: Option<String>,
    #[serde(default)]
    head: Option<String>,
}

enum Backend {
    Jsonl(TraceEventStore),
    #[cfg(feature = "tritdb")]
    TritDb(Box<TritDbTraceStore>),
}
impl Backend {
    fn append(&self, event: &TraceEvent) -> Result<(), Error> {
        match self {
            Self::Jsonl(store) => store.append(event),
            #[cfg(feature = "tritdb")]
            Self::TritDb(store) => store.append(event),
        }
    }

    fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        match self {
            Self::Jsonl(store) => store.load(),
            #[cfg(feature = "tritdb")]
            Self::TritDb(store) => store.load(),
        }
    }

    fn load_log(&self, trace_id: &str) -> Result<TraceLog, Error> {
        match self {
            Self::Jsonl(store) => store.load_log(trace_id),
            #[cfg(feature = "tritdb")]
            Self::TritDb(store) => store.load_log(trace_id),
        }
    }
}
fn arg_value(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}

fn backend_name() -> String {
    arg_value("--trace-backend").unwrap_or_else(|| "jsonl".to_string())
}
pub(crate) fn run(input: &str) -> Result<Value, Error> {
    let mut request: TraceInput = crate::parse(input)?;
    let path = arg_value("--trace-path")
        .or_else(|| request.path.clone())
        .ok_or_else(|| Error::InputParse {
            message: "trace 需要 path 或 --trace-path".into(),
        })?;
    request.path = Some(path.clone());
    let backend = backend_name();
    if matches!(request.action.as_str(), "sql" | "snapshot" | "restore") && backend != "tritdb" {
        return Err(Error::InputParse {
            message: format!("trace {} 需要 tritdb backend", request.action),
        });
    }
    match backend.as_str() {
        "jsonl" => run_with(Backend::Jsonl(TraceEventStore::new(&path)), &request),
        #[cfg(feature = "tritdb")]
        "tritdb" => run_with(
            Backend::TritDb(Box::new(TritDbTraceStore::open(&path)?)),
            &request,
        ),
        other => Err(Error::InputParse {
            message: format!("未知 trace backend {other:?}（可用：jsonl/tritdb）"),
        }),
    }
}

fn run_with(backend: Backend, request: &TraceInput) -> Result<Value, Error> {
    match request.action.as_str() {
        "append" => {
            for event in &request.events {
                backend.append(event)?;
            }
            Ok(json!({
                "ok": true,
                "backend": backend_name(),
                "appended": request.events.len(),
            }))
        }
        "load" => {
            let events = backend.load()?;
            Ok(json!({
                "ok": true,
                "backend": backend_name(),
                "events": events,
            }))
        }
        "check" => {
            let trace_id = request
                .trace_id
                .as_deref()
                .ok_or_else(|| Error::InputParse {
                    message: "trace check 需要 trace_id".into(),
                })?;
            let log = backend.load_log(trace_id)?;
            let report = check_trace(&log);
            Ok(json!({
                "ok": true,
                "backend": backend_name(),
                "clean": report.is_clean(),
                "brief": report.brief(),
                "violations": report.violations,
            }))
        }
        "sql" => sql_action(request),
        "snapshot" => snapshot_action(request),
        "restore" => restore_action(request),
        other => Err(Error::InputParse {
            message: format!(
                "未知 trace action {other:?}（可用：append/load/check/sql/snapshot/restore）"
            ),
        }),
    }
}

#[cfg(feature = "tritdb-sql")]
fn sql_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace sql 需要 path".into(),
    })?;
    let sql = request.sql.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace sql 需要 sql".into(),
    })?;
    let results = trit_orch::tritdb_backend::sql::query_trace_events(Path::new(path), sql)?;
    Ok(json!({
        "ok": true,
        "backend": "tritdb",
        "results": results,
    }))
}

#[cfg(not(feature = "tritdb-sql"))]
fn sql_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace sql 需要编译 feature tritdb-sql".into(),
    })
}

#[cfg(feature = "tritdb-cas")]
fn snapshot_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace snapshot 需要 path".into(),
    })?;
    let cas_root = request
        .cas_root
        .as_deref()
        .ok_or_else(|| Error::InputParse {
            message: "trace snapshot 需要 cas_root".into(),
        })?;
    let head = trit_orch::tritdb_backend::cas::snapshot_wal(Path::new(path), Path::new(cas_root))?;
    Ok(json!({
        "ok": true,
        "backend": "tritdb",
        "head": head,
    }))
}

#[cfg(not(feature = "tritdb-cas"))]
fn snapshot_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace snapshot 需要编译 feature tritdb-cas".into(),
    })
}

#[cfg(feature = "tritdb-cas")]
fn restore_action(request: &TraceInput) -> Result<Value, Error> {
    let path = request.path.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace restore 需要 path".into(),
    })?;
    let cas_root = request
        .cas_root
        .as_deref()
        .ok_or_else(|| Error::InputParse {
            message: "trace restore 需要 cas_root".into(),
        })?;
    let head = request.head.as_deref().ok_or_else(|| Error::InputParse {
        message: "trace restore 需要 head".into(),
    })?;
    trit_orch::tritdb_backend::cas::restore_wal(Path::new(path), Path::new(cas_root), head)?;
    Ok(json!({
        "ok": true,
        "backend": "tritdb",
        "restored": head,
    }))
}

#[cfg(not(feature = "tritdb-cas"))]
fn restore_action(_request: &TraceInput) -> Result<Value, Error> {
    Err(Error::InputParse {
        message: "trace restore 需要编译 feature tritdb-cas".into(),
    })
}
