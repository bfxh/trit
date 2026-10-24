//! 跨进程存储锁：目录原子抢占 + 持有者记录 + 陈旧锁恢复。
//!
//! 抢占靠 `create_dir` 的原子性；拿到锁后把 pid / 主机名 / 获取时间写进锁目录的
//! `owner.json`。进程被 kill、掉电或崩溃时 `Drop` 不会执行，锁会残留——所以
//! [`super::FileCheckpointStore::break_stale_lock`] 会用**平台自带命令**探测持有进程
//! 是否还活着，只清理确定已死的锁；探测失败一律当作活着（宁可超时，不误杀活锁）。
//!
//! 局限（如实记录）：
//! - pid 可能被系统复用，极端情况下会把死进程的 pid 认成活进程（保守方向，不会误杀）；
//! - 探测依赖 `ps`（unix）/ `tasklist`（Windows），命令缺失时不恢复，只能人工删锁目录；
//! - 跨主机共享目录上 pid 无意义，`host` 不同即视为占有，从不自动清理。

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// 锁目录里的持有者记录文件名。
pub(crate) const OWNER_FILE: &str = "owner.json";

/// 没有 `owner.json` 的锁目录至少这么旧才视为陈旧。
///
/// 覆盖 `create_dir` 成功、`owner.json` 还没落盘就被 kill 的窗口；短于此窗口的
/// 无主锁一律不碰，避免把刚开始初始化的新锁当成残留。
pub(crate) const OWNERLESS_STALE_AFTER: Duration = Duration::from_secs(30);

/// 锁目录内的持有者信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LockOwner {
    /// 持有进程 id。
    pub pid: u32,
    /// 持有主机名（拿不到时为空串）。
    pub host: String,
    /// 获取锁的时间（Unix 毫秒），仅供诊断。
    pub acquired_unix_ms: u64,
}

/// 跨进程存储锁；`Drop` 时删除锁目录。
#[derive(Debug)]
pub struct CheckpointLock {
    pub(super) path: PathBuf,
    pub(super) owner: LockOwner,
}

impl CheckpointLock {
    /// 锁目录路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 持有者 pid。
    pub fn owner_pid(&self) -> u32 {
        self.owner.pid
    }

    /// 持有主机名（空串表示取不到）。
    pub fn owner_host(&self) -> &str {
        &self.owner.host
    }
}

impl Drop for CheckpointLock {
    fn drop(&mut self) {
        // 只删仍是自己的那把：陈旧回收/人工删锁之后，同路径可能已是别人的锁目录。
        if read_owner(&self.path).as_ref() == Some(&self.owner) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

/// 当前进程的持有者记录。
pub(crate) fn local_owner() -> LockOwner {
    LockOwner {
        pid: std::process::id(),
        host: local_host(),
        acquired_unix_ms: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64),
    }
}

/// 写持有者记录（`owner.json`）。
pub(crate) fn write_owner(dir: &Path, owner: &LockOwner) -> std::io::Result<()> {
    let bytes =
        serde_json::to_vec(owner).map_err(|error| std::io::Error::other(error.to_string()))?;
    // `create_new`：目录虽由我们创建，但可能已被判为陈旧并回收重建——
    // 这种情况下绝不能覆盖新持有者的记录，只能失败让调用方退让。
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(OWNER_FILE))?;
    file.write_all(&bytes)
}

/// 读持有者记录；缺失或损坏返回 `None`（损坏按无主处理，由年龄兜底）。
pub(crate) fn read_owner(dir: &Path) -> Option<LockOwner> {
    let bytes = fs::read(dir.join(OWNER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 本机主机名；两个平台变量都缺时为空串（空串两两相等，仍按本机处理）。
fn local_host() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default()
}

/// 探测进程是否存活：`Some(true/false)` 为确定结论，`None` 表示探测不可用。
pub(crate) fn process_alive(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .ok()?;
        if !output.status.success() {
            return Some(false);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        Some(
            text.split_whitespace()
                .any(|token| token == pid.to_string()),
        )
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        // 找不到时输出是本地化提示文本，不会有等于 pid 的纯数字列。
        let text = String::from_utf8_lossy(&output.stdout);
        Some(
            text.split_whitespace()
                .any(|token| token == pid.to_string()),
        )
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        None
    }
}

/// 目录年龄（按 mtime）；取不到返回 `None`。
pub(crate) fn dir_age(dir: &Path) -> Option<Duration> {
    let modified = fs::metadata(dir).ok()?.modified().ok()?;
    SystemTime::now().duration_since(modified).ok()
}
