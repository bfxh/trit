use serde::{Deserialize, Serialize};

/// 一次执行的结束方式。`Signalled`/`TimedOut`/`SpawnFailed` 都不是断言失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exit {
    Completed { code: i32 },
    Signalled,
    TimedOut,
    SpawnFailed,
}
