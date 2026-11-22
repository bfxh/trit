//! tritdb 后端的动作路由：`sql`/`snapshot`/`restore`/`serve` 各自打开自己的存储。
//!
//! 不能先建一个 `TritDbTraceStore` 再让 action 二次打开同一个 WAL：
//! `tritdb-txn` 的 WAL 锁是进程级排他的，同进程双持会报 `wal is locked by process`。

use serde_json::Value;
use trit_orch::Error;

use super::TritDbTraceStore;

use super::actions::{restore_action, serve_action, snapshot_action, sql_action};
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
        other => Err(Error::InputParse {
            message: format!("未知 trace action {other:?}"),
        }),
    }
}
