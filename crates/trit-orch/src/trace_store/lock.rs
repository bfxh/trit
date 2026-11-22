//! 跨进程写锁：日志路径旁的 `.lock` 文件（create_new 原子创建）。
//!
//! 进程内先按路径分锁，再争抢 `.lock`；崩溃留下的锁文件超过 stale 时间会被回收。
//! 这是 advisory 锁：所有写者都必须经过 `TraceEventStore` 才有效。

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Error;

static LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

const LOCK_STALE: Duration = Duration::from_secs(300);
const LOCK_TIMEOUT: Duration = Duration::from_secs(60);

/// 进程内按路径分锁；Weak 表在每次取锁时清理已释放条目。
pub(super) fn path_lock(path: &Path) -> Arc<Mutex<()>> {
    let map = LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut guard = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.retain(|_, weak| weak.strong_count() > 0);
    if let Some(lock) = guard.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    guard.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

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
                    let _ = writeln!(file, "{}\n{}", std::process::id(), now_secs());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if stale(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() >= LOCK_TIMEOUT {
                        return Err(Error::TraceIo {
                            path: path.display().to_string(),
                            message: "timed out waiting for cross-process trace lock".into(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(10));
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
        let _ = std::fs::remove_file(&self.path);
    }
}

fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|age| age >= LOCK_STALE)
        .unwrap_or(false)
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
