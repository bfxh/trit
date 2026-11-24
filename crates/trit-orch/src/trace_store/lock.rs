//! 跨进程写锁：日志路径旁的 `.lock` 文件（create_new 原子创建）。
//!
//! 进程内先按路径分锁，再争抢 `.lock`；崩溃留下的锁文件超过 stale 时间会被回收。
//! 这是 advisory 锁：所有写者都必须经过 `TraceEventStore` 才有效。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::Error;

#[path = "lock_support.rs"]
mod support;
pub(super) use support::path_lock;

const LOCK_STALE: Duration = Duration::from_secs(300);
const LOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// 跨进程锁的 RAII guard；Drop 时删除 `.lock`。
pub(crate) struct FileLock {
    path: PathBuf,
}

impl FileLock {
    pub(crate) fn acquire(log: &Path) -> Result<Self, Error> {
        let mut name = log.as_os_str().to_os_string();
        name.push(".lock");
        let path = PathBuf::from(name);
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| Error::TraceIo {
                path: parent.display().to_string(),
                message: error.to_string(),
            })?;
        }
        let started = Instant::now();
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = writeln!(file, "{}\n{}", std::process::id(), support::now_secs());
                    return Ok(Self { path });
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists
                        || (cfg!(windows)
                            && error.kind() == std::io::ErrorKind::PermissionDenied) =>
                {
                    if support::permission_denied_is_fatal(error.kind(), &path, started) {
                        return Err(Error::TraceIo {
                            path: path.display().to_string(),
                            message: error.to_string(),
                        });
                    }
                    if support::stale(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(Error::TraceIo {
                            path: path.display().to_string(),
                            message: "timed out waiting for cross-process trace lock".into(),
                        });
                    }
                    std::thread::sleep(support::RETRY);
                }
                Err(error) => {
                    return Err(Error::TraceIo {
                        path: path.display().to_string(),
                        message: error.to_string(),
                    });
                }
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        support::remove_with_retry(&self.path);
    }
}
