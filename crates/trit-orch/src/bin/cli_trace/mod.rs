use serde::Deserialize;
use serde_json::{Value, json};
#[cfg(feature = "tritdb")]
use trit_orch::tritdb_backend::TritDbTraceStore;
use trit_orch::{Error, TraceBackend, TraceEvent, TraceEventStore, check_trace};

#[cfg(feature = "tritdb")]
#[path = "route.rs"]
mod route;

mod actions;
mod actions_gc;
mod actions_sync;

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
    #[serde(default)]
    listen: Option<String>,
}

type Backend = Box<dyn TraceBackend>;
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
            message: "trace 需要 path".into(),
        })?;
    request.path = Some(path.clone());
    let backend = backend_name();
    if matches!(
        request.action.as_str(),
        "sql" | "snapshot" | "restore" | "serve"
    ) && backend != "tritdb"
    {
        return Err(Error::InputParse {
            message: format!("trace {} 需要 tritdb backend", request.action),
        });
    }
    match backend.as_str() {
        "jsonl" => run_with(Box::new(TraceEventStore::new(&path)), &request),
        #[cfg(feature = "tritdb")]
        "tritdb" => route::run(&path, &request),
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
        other => Err(Error::InputParse {
            message: format!(
                "未知 trace action {other:?}（可用：append/load/check/sql/snapshot/restore/serve）"
            ),
        }),
    }
}
