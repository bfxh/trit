//! pgwire 服务：启动前物化一次，之后按 `TRIT_TRACE_SYNC_SECS` 定时刷新。
//!
//! 刷新线程与服务的查询引擎并发写同一个 WAL，靠 `tritdb-txn` 的事务冲突重试；
//! 连续失败只记日志，不影响已建立的服务。

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::Error;

use crate::tritdb_backend::sql::sync_trace_table;

/// 默认刷新间隔（秒）；`0` 关闭后台刷新。
const DEFAULT_SYNC_SECS: u64 = 5;
const ATTEMPTS: u32 = 3;

/// 在 `listen` 上启动 pgwire；阻塞直到服务退出。
pub fn serve_trace(db: &Path, listen: &str) -> Result<(), Error> {
    sync_trace_table(db)?;
    if let Some(interval) = refresh_interval() {
        spawn_refresh(db.to_path_buf(), interval);
    }
    let listener = TcpListener::bind(listen).map_err(|error| io(listen, error))?;
    tritdb_pgwire::serve(db.to_path_buf(), listener)
        .map_err(|error| io(&db.display().to_string(), error))
}

fn refresh_interval() -> Option<Duration> {
    match std::env::var("TRIT_TRACE_SYNC_SECS") {
        Ok(text) => parse_interval(&text),
        Err(_) => Some(Duration::from_secs(DEFAULT_SYNC_SECS)),
    }
}

fn parse_interval(text: &str) -> Option<Duration> {
    let secs: u64 = text.trim().parse().ok()?;
    (secs > 0).then(|| Duration::from_secs(secs))
}

fn spawn_refresh(db: PathBuf, interval: Duration) {
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(interval);
            sync_with_retry(&db);
        }
    });
}

/// 与查询并发时事务会冲突；短退避重试，仍失败只记日志。
fn sync_with_retry(db: &Path) {
    let mut delay = Duration::from_millis(50);
    for attempt in 1..=ATTEMPTS {
        match sync_trace_table(db) {
            Ok(()) => return,
            Err(_) if attempt < ATTEMPTS => {
                std::thread::sleep(delay);
                delay *= 2;
            }
            Err(error) => eprintln!("trace sync 失败（{}）：{error}", db.display()),
        }
    }
}

fn io(path: &str, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.to_string(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_interval;

    #[test]
    fn interval_parsing_rejects_zero_and_garbage() {
        assert_eq!(parse_interval("5").map(|value| value.as_secs()), Some(5));
        assert_eq!(parse_interval(" 7 ").map(|value| value.as_secs()), Some(7));
        assert!(parse_interval("0").is_none());
        assert!(parse_interval("").is_none());
        assert!(parse_interval("soon").is_none());
    }
}
