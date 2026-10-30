//! `--timeout-ms` 的自包装：重新执行自身并在超时后杀掉整个子进程。
//!
//! 从 `main.rs` 拆出（`trit-cli` 的 `main.rs` 有 800 行上帝模块上限）。

use anyhow::{Context, Result};

/// 重新执行自身，并在 `timeout_ms` 后杀掉整个子进程。
///
/// 子进程通过 `TRIT_TIMEOUT_CHILD` 跳过包装，避免递归；输出直接继承终端。
pub(crate) fn run_with_timeout(timeout_ms: u64) -> Result<()> {
    let exe = std::env::current_exe().context("无法定位 trit 可执行文件")?;
    let mut child = std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env("TRIT_TIMEOUT_CHILD", "1")
        .spawn()
        .context("无法启动超时子进程")?;
    let started = std::time::Instant::now();
    let limit = std::time::Duration::from_millis(timeout_ms);
    loop {
        if let Some(status) = child.try_wait()? {
            std::process::exit(status.code().unwrap_or(1));
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("超时：{timeout_ms} ms 内未完成，已终止");
            std::process::exit(124);
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
