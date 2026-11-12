//! CLI 写锁：对 `--out` / `--log` 路径加 advisory 锁文件，防止多进程并发写坏。
//!
//! 锁文件用 `create_new`（`O_EXCL`）原子创建，内容记录 pid 与 unix_ms。进程退出时
//! `Drop` 删除；崩溃遗留的锁超过 [`STALE_MS`] 后可被回收。`atomic_write` 防的是
//! 半写，锁防的是两个读-改-写事务互相覆盖（lost update）。

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_WAIT_MS: u64 = 2_000;
const STALE_MS: u128 = 600_000;
const POLL_MS: u64 = 50;

/// 一个持有中的锁；`Drop` 时只删除自己创建的锁文件。
pub(crate) struct ArtifactLock {
    path: PathBuf,
    pid: u32,
}

impl Drop for ArtifactLock {
    fn drop(&mut self) {
        if let Ok(text) = fs::read_to_string(&self.path)
            && text.contains(&format!("pid={}", self.pid))
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// 扫描 CLI 参数里的 `--out` / `--log`，按路径字典序加锁（避免死锁）。
pub(crate) fn acquire_cli(args: &[String]) -> Result<Vec<ArtifactLock>, String> {
    let mut paths = Vec::new();
    let mut index = 0;
    while index < args.len() {
        if matches!(args[index].as_str(), "--out" | "--log") {
            if let Some(raw) = args.get(index + 1)
                && raw != "-"
                && !raw.is_empty()
            {
                let path = PathBuf::from(raw);
                paths.push(std::path::absolute(&path).unwrap_or(path));
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    paths.sort();
    paths.dedup();
    let wait = Duration::from_millis(wait_ms());
    let mut locks = Vec::with_capacity(paths.len());
    for path in paths {
        locks.push(acquire(&path, wait)?);
    }
    Ok(locks)
}

fn acquire(path: &Path, wait: Duration) -> Result<ArtifactLock, String> {
    let lock_path = lock_path(path);
    let pid = std::process::id();
    let started = SystemTime::now();
    loop {
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(mut file) => {
                let _ = writeln!(file, "pid={pid}");
                let _ = writeln!(file, "unix_ms={}", unix_ms());
                let _ = file.sync_all();
                return Ok(ArtifactLock {
                    path: lock_path,
                    pid,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if stale(&lock_path) {
                    let _ = fs::remove_file(&lock_path);
                    continue;
                }
                if started.elapsed().unwrap_or_default() >= wait {
                    let holder = fs::read_to_string(&lock_path).unwrap_or_default();
                    return Err(format!(
                        "{} 被另一个进程锁定（{}）：等待 {} ms 超时（锁文件 {}）",
                        path.display(),
                        holder.trim().replace('\n', " "),
                        wait.as_millis(),
                        lock_path.display()
                    ));
                }
                thread::sleep(Duration::from_millis(POLL_MS));
            }
            Err(error) => return Err(format!("{}: {error}", lock_path.display())),
        }
    }
}

fn wait_ms() -> u64 {
    std::env::var("TRIT_LOCK_WAIT_MS")
        .ok()
        .and_then(|raw| raw.parse().ok())
        .unwrap_or(DEFAULT_WAIT_MS)
}

fn lock_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn stale(lock_path: &Path) -> bool {
    fs::metadata(lock_path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age.as_millis() > STALE_MS)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis())
}
