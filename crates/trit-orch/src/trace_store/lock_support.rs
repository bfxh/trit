//! Support state for the cross-process trace lock: in-process path locks,
//! stale detection, and the Windows delete-pending retry shape.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use std::io;

static LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();

pub(super) const RETRY: Duration = Duration::from_millis(10);
const DENIED_GRACE: Duration = Duration::from_secs(2);

/// 进程内按路径分锁；Weak 表在每次取锁时清理已释放条目。
pub(crate) fn path_lock(path: &Path) -> Arc<Mutex<()>> {
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

pub(super) fn stale(path: &Path) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|age| age >= super::LOCK_STALE)
        .unwrap_or(false)
}

pub(super) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// True only when a Windows `PermissionDenied` cannot be a delete-pending
/// window: the path is invisible *and* the grace period has elapsed. A real
/// ACL denial therefore fails closed instead of spinning to LOCK_TIMEOUT.
pub(super) fn permission_denied_is_fatal(
    kind: io::ErrorKind,
    path: &Path,
    started: Instant,
) -> bool {
    cfg!(windows)
        && kind == io::ErrorKind::PermissionDenied
        && !path.exists()
        && started.elapsed() >= DENIED_GRACE
}

/// Remove the lock on drop. On Windows a just-closed entry can stay
/// delete-pending for a short window, so retry briefly; on other platforms a
/// single best-effort unlink is enough.
pub(super) fn remove_with_retry(path: &Path) {
    if cfg!(windows) {
        for _ in 0..20 {
            match std::fs::remove_file(path) {
                Ok(()) => return,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return,
                Err(_) => std::thread::sleep(RETRY),
            }
        }
    }
    let _ = std::fs::remove_file(path);
}
