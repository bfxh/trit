//! tritdb 后端动作路由：各 action 各自打开存储，避免同进程双持 WAL 锁。
use super::TritDbTraceStore;
use serde_json::Value;
use trit_orch::Error;

use super::actions::{restore_action, serve_action, snapshot_action, sql_action};
use super::actions_gc::{compact_action, gc_action};
use super::actions_sync::sync_action;
use super::{Backend, TraceInput, run_with};

pub(super) fn run(path: &str, request: &TraceInput) -> Result<Value, Error> {
    match request.action.as_str() {
        "append" | "load" | "check" => run_with(
            Backend::TritDb(Box::new(TritDbTraceStore::open(path)?)),
            request,
        ),
        "sql" => sql_action(request),
        "snapshot" => snapshot_action(request),
        "restore" => restore_action(request),
        "serve" => serve_action(request),
        "sync" => sync_action(request),
        "compact" => compact_action(request),
        "gc" => gc_action(request),
        other => Err(Error::InputParse {
            message: format!("未知 trace action {other:?}"),
        }),
    }
}
