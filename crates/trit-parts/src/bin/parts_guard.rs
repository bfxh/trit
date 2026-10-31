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

use std::io::{Read, Write};
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
    /// 是否响铃/弹窗（默认关；agent-first，状态靠 status/RUNS 文件）
    notify_on: bool,
    /// 事件登记表（Markdown，追加写；智能体靠它知道「跑完了」）。
    registry: Option<PathBuf>,
    /// 磁盘下限（GB）：低于它就拒绝启动/中途停掉。
    disk_min_gb: f64,
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
        notify_on: false,
        registry: None,
        disk_min_gb: 5.0,
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
            "--notify" => {
                a.notify_on = true;
                i += 1;
            }
            "--registry" => {
                a.registry = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--disk-min-gb" => {
                a.disk_min_gb = argv[i + 1].parse().unwrap_or(5.0);
                i += 2;
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

/// 目标盘剩余 GB（Windows 用 PowerShell 取；失败返回 None=不判）。
fn free_gb(dir: &Path) -> Option<f64> {
    #[cfg(windows)]
    {
        let drive = dir
            .components()
            .next()
            .map(|c| {
                c.as_os_str()
                    .to_string_lossy()
                    .trim_end_matches(':')
                    .to_string()
            })
            .unwrap_or_default();
        let out = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-PSDrive -Name {drive} | Select-Object -ExpandProperty Free)/1GB"),
            ])
            .output()
            .ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        s.trim().parse::<f64>().ok()
    }
    #[cfg(not(windows))]
    {
        let _ = dir;
        None
    }
}

fn append_registry(path: &Option<PathBuf>, run: &str, state: &str, detail: &str) {
    let Some(p) = path else { return };
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).ok();
    }
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(p)
    {
        let _ = writeln!(f, "- {} `{}` **{}** {}", now_hms(), run, state, detail);
    }
    // 只留最近 200 行，避免无限增长
    if let Ok(txt) = std::fs::read_to_string(p) {
        let lines: Vec<&str> = txt.lines().collect();
        if lines.len() > 200 {
            let keep = lines[lines.len() - 200..].join(
                "
",
            ) + "
";
            std::fs::write(p, keep).ok();
        }
    }
}

/// 写状态文件 + 追加到登记表（`RUNS.md`），智能体下次开工就能看到。
fn event(a: &Args, state: &str, detail: &str) {
    write_status(&a.status, state, detail);
    let run = a
        .status
        .as_ref()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "run".to_string());
    append_registry(&a.registry, &run, state, detail);
}

fn log_mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).ok()?.modified().ok()
}

fn spawn(cmd: &[String], log: Option<&Path>) -> std::io::Result<Child> {
    if cmd.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "缺少训练命令（放在 -- 之后）",
        ));
    }
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    if let Some(p) = log {
        let mut child = c.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
        if let Some(out) = child.stdout.take() {
            tee(out, p.to_path_buf());
        }
        if let Some(err) = child.stderr.take() {
            tee(err, p.to_path_buf());
        }
        Ok(child)
    } else {
        c.stdout(Stdio::inherit()).stderr(Stdio::inherit()).spawn()
    }
}

/// 把子进程输出原样追加进日志——日志 mtime 就是看门狗的心跳源。
fn tee(r: impl Read + Send + 'static, log: PathBuf) {
    std::thread::spawn(move || {
        let mut r = std::io::BufReader::new(r);
        let mut buf = [0u8; 4096];
        loop {
            match r.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log)
                    {
                        let _ = f.write_all(&buf[..n]);
                        let _ = f.flush();
                    }
                }
            }
        }
    });
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

/// `parts-guard wait --status <json> [--timeout-sec N]`：
/// 阻塞到训练进入终态，打印并返回退出码（0=DONE，1=失败，3=超时）。给智能体同步用。
fn wait_cmd(argv: &[String]) -> ! {
    let mut status: Option<PathBuf> = None;
    let mut timeout = 0u64;
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--status" => {
                status = Some(PathBuf::from(&argv[i + 1]));
                i += 2;
            }
            "--timeout-sec" => {
                timeout = argv[i + 1].parse().unwrap_or(0);
                i += 2;
            }
            other => {
                eprintln!("wait: 未知参数 {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(p) = status else {
        eprintln!("wait 需要 --status <json>");
        std::process::exit(2);
    };
    let t0 = Instant::now();
    loop {
        if let Ok(txt) = std::fs::read_to_string(&p) {
            let parts: Vec<&str> = txt.split('"').collect();
            let st = parts
                .windows(3)
                .find(|w| w[0] == "state" && w[1] == ":")
                .map(|w| {
                    w[2].trim_matches(|c| c == ',' || c == '}' || c == ' ')
                        .to_string()
                })
                .unwrap_or_default();
            match st.as_str() {
                "DONE" => {
                    println!("DONE");
                    std::process::exit(0);
                }
                "FAILED" | "STALL_STOP" | "DISK_LOW" | "SPAWN_FAIL" | "WAIT_ERR" => {
                    println!("{st}");
                    std::process::exit(1);
                }
                "RUN" | "RESTART" | "ATTACH" | "STALL" => {
                    println!("{st} …");
                }
                _ => {}
            }
        }
        if timeout > 0 && t0.elapsed() > Duration::from_secs(timeout) {
            println!("TIMEOUT");
            std::process::exit(3);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("wait") {
        wait_cmd(&argv[1..]);
    }
    let a = parse();
    // 磁盘预检：宁可不跑，也不要跑出一堆 0 字节产物
    let disk_dir = a
        .status
        .as_ref()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    if let Some(g) = free_gb(&disk_dir) {
        if g < a.disk_min_gb {
            event(
                &a,
                "DISK_LOW",
                &format!("剩余 {g:.1} GB < 下限 {:.1} GB", a.disk_min_gb),
            );
            notify(
                "parts-guard: 磁盘不足",
                &format!("剩余 {g:.1} GB，未启动"),
                !a.notify_on,
            );
            std::process::exit(2);
        }
        println!(
            "[guard {}] 磁盘剩余 {g:.1} GB（下限 {:.1}）",
            now_hms(),
            a.disk_min_gb
        );
    }
    let mut attempt = 0u32;
    loop {
        // 1) 拿到被托管的进程
        let mut child: Option<Child> = None;
        let watched_pid: u32;
        if let Some(pid) = a.attach {
            watched_pid = pid;
            event(&a, "ATTACH", &format!("pid={pid}"));
        } else {
            match spawn(&a.cmd, a.log.as_deref()) {
                Ok(c) => {
                    watched_pid = c.id();
                    child = Some(c);
                    event(&a, "RUN", &format!("pid={watched_pid}"));
                }
                Err(e) => {
                    event(&a, "SPAWN_FAIL", &e.to_string());
                    notify("parts-guard", &format!("训练启动失败: {e}"), !a.notify_on);
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
                        event(&a, "WAIT_ERR", &e.to_string());
                        break;
                    }
                }
            } else if !alive_pid(watched_pid) {
                break;
            }
            // 磁盘中途变红：杀掉，别写出半截数据
            if a.disk_min_gb > 0.0
                && a.status.is_some()
                && std::time::SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .is_multiple_of(30)
                && let Some(g) = free_gb(&disk_dir)
                && g < a.disk_min_gb
            {
                event(&a, "DISK_LOW", &format!("剩余 {g:.1} GB < 下限，已杀"));
                if let Some(c) = child.as_mut() {
                    let _ = c.kill();
                }
                notify(
                    "parts-guard: 磁盘不足",
                    &format!("剩余 {g:.1} GB，已停止"),
                    !a.notify_on,
                );
                std::process::exit(2);
            }
            if a.log.is_some() && last_change.elapsed() > stall {
                stalled = true;
                event(
                    &a,
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
                    !a.notify_on,
                );
                break;
            }
        }

        // 3) 判结果
        let failed = stalled || exit_code.map(|c| c != 0).unwrap_or(false);
        if stalled && attempt >= a.retries {
            event(&a, "STALL_STOP", "卡死且重试用尽");
            notify("parts-guard: 训练卡死", "已停止（重试用尽）", !a.notify_on);
            std::process::exit(1);
        }
        if !failed {
            event(&a, "DONE", "训练结束");
            notify(
                "parts-guard: 训练结束",
                "进程已停止，GPU 已释放",
                !a.notify_on,
            );
            return;
        }
        if attempt < a.retries {
            attempt += 1;
            event(&a, "RESTART", &format!("第 {attempt} 次重试"));
            notify(
                "parts-guard: 自动续跑",
                &format!("第 {attempt} 次重试（从断点继续）"),
                !a.notify_on,
            );
            continue;
        }
        event(&a, "FAILED", "重试用尽或崩溃");
        notify("parts-guard: 训练失败", "重试用尽，已停止", !a.notify_on);
        std::process::exit(1);
    }
}
