//! `parts-guard`：训练看门狗。
//!
//! 两件事：
//! 1. **托住训练**：训练一结束（正常/崩溃）就立刻停下、写状态、响铃/通知；
//! 2. **判卡死**：日志文件超过 `--stall-sec` 没更新就杀掉进程并报 STALL；
//!    崩溃时若给了 `--retries`，自动用同一命令重启（配合 `--state DIR` 从断点续跑）。
//!
//! 用法（把训练命令放在 `--` 之后）：
//! ```text
//! parts-guard --stall-sec 900 --retries 3 --status out/status.json -- \
//!   parts-train --state out --steps 60000 ...
//! ```
//! 也可以 `--attach <pid> --log <path>` 托管一个已经在跑的进程。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

struct Args {
    stall_sec: u64,
    poll_sec: u64,
    retries: u32,
    status: Option<PathBuf>,
    log: Option<PathBuf>,
    attach: Option<u32>,
    quiet: bool,
    cmd: Vec<String>,
}

fn parse() -> Args {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut a = Args {
        stall_sec: 900,
        poll_sec: 5,
        retries: 0,
        status: None,
        log: None,
        attach: None,
        quiet: false,
        cmd: Vec::new(),
    };
    let mut i = 0;
    while i < argv.len() {
        let key = argv[i].as_str();
        match key {
            "--stall-sec" => {
                a.stall_sec = argv[i + 1].parse().unwrap_or(900);
                i += 2;
            }
            "--poll-sec" => {
                a.poll_sec = argv[i + 1].parse().unwrap_or(5);
                i += 2;
            }
            "--retries" => {
                a.retries = argv[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            "--status" => {
                a.status = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--log" => {
                a.log = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--attach" => {
                a.attach = argv[i + 1].parse().ok();
                i += 2;
            }
            "--quiet" => {
                a.quiet = true;
                i += 1;
            }
            "--" => {
                a.cmd = argv[i + 1..].to_vec();
                break;
            }
            other => {
                eprintln!("未知参数 {other}");
                std::process::exit(2);
            }
        }
    }
    a
}

fn now_hms() -> String {
    let d = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let s = d.as_secs() % 86400;
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn write_status(path: &Option<PathBuf>, state: &str, detail: &str) {
    println!("[guard {}] {state} {detail}", now_hms());
    if let Some(p) = path {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        let body = format!(
            "{{\"state\":\"{state}\",\"time\":\"{}\",\"detail\":\"{}\"}}\n",
            now_hms(),
            detail.replace('"', "'")
        );
        std::fs::write(p, body).ok();
    }
}

/// 响铃 + 尽力弹一个系统提示（Windows：msg；失败则忽略）。
fn notify(title: &str, body: &str, quiet: bool) {
    if quiet {
        return;
    }
    #[cfg(windows)]
    {
        let _ = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "[console]::beep(1200,300); [console]::beep(800,400)",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let msg = format!("title={title}; body={body}");
        let _ = Command::new("msg")
            .args(["*", "/TIME:60", &msg])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("sh").args(["-c", "printf '\\a'"]).status();
    }
}

fn log_mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

fn spawn(cmd: &[String]) -> std::io::Result<Child> {
    if cmd.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "缺少训练命令（放在 -- 之后）",
        ));
    }
    Command::new(&cmd[0])
        .args(&cmd[1..])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
}

fn alive_pid(pid: u32) -> bool {
    #[cfg(windows)]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output();
        match out {
            Ok(o) => String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()),
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn main() {
    let a = parse();
    let mut attempt = 0u32;
    loop {
        // 1) 拿到被托管的进程
        let mut child: Option<Child> = None;
        let watched_pid: u32;
        if let Some(pid) = a.attach {
            watched_pid = pid;
            write_status(&a.status, "ATTACH", &format!("pid={pid}"));
        } else {
            match spawn(&a.cmd) {
                Ok(c) => {
                    watched_pid = c.id();
                    child = Some(c);
                    write_status(&a.status, "RUN", &format!("pid={watched_pid}"));
                }
                Err(e) => {
                    write_status(&a.status, "SPAWN_FAIL", &e.to_string());
                    notify("parts-guard", &format!("训练启动失败: {e}"), a.quiet);
                    std::process::exit(1);
                }
            }
        }

        // 2) 监督循环
        let mut last_seen = SystemTime::now();
        let mut last_change = Instant::now();
        let poll = Duration::from_secs(a.poll_sec.max(1));
        let stall = Duration::from_secs(a.stall_sec.max(10));
        let mut exit_code: Option<i32> = None;
        let mut stalled = false;
        loop {
            std::thread::sleep(poll);
            if let Some(lp) = &a.log
                && let Some(m) = log_mtime(lp)
                && m != last_seen
            {
                last_seen = m;
                last_change = Instant::now();
            }
            if let Some(c) = child.as_mut() {
                match c.try_wait() {
                    Ok(Some(st)) => {
                        exit_code = st.code();
                        break;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        write_status(&a.status, "WAIT_ERR", &e.to_string());
                        break;
                    }
                }
            } else if !alive_pid(watched_pid) {
                break;
            }
            if a.log.is_some() && last_change.elapsed() > stall {
                stalled = true;
                write_status(
                    &a.status,
                    "STALL",
                    &format!("pid={watched_pid} 超过 {}s 无日志更新，已杀", a.stall_sec),
                );
                if let Some(c) = child.as_mut() {
                    let _ = c.kill();
                } else {
                    #[cfg(windows)]
                    kill_pid(watched_pid);
                }
                notify(
                    "parts-guard: 训练卡死",
                    &format!("超过 {}s 无进度，已停止", a.stall_sec),
                    a.quiet,
                );
                break;
            }
        }

        // 3) 判结果
        let failed = stalled || exit_code.map(|c| c != 0).unwrap_or(false);
        if stalled && attempt >= a.retries {
            write_status(&a.status, "STALL_STOP", "卡死且重试用尽");
            notify("parts-guard: 训练卡死", "已停止（重试用尽）", a.quiet);
            std::process::exit(1);
        }
        if !failed {
            write_status(&a.status, "DONE", "训练结束");
            notify("parts-guard: 训练结束", "进程已停止，GPU 已释放", a.quiet);
            return;
        }
        if attempt < a.retries {
            attempt += 1;
            write_status(&a.status, "RESTART", &format!("第 {attempt} 次重试"));
            notify(
                "parts-guard: 自动续跑",
                &format!("第 {attempt} 次重试（从断点继续）"),
                a.quiet,
            );
            continue;
        }
        write_status(&a.status, "FAILED", "重试用尽或崩溃");
        notify("parts-guard: 训练失败", "重试用尽，已停止", a.quiet);
        std::process::exit(1);
    }
}
