//! 采集器（Windows）：起目标进程 + 轮询进程表 + 结束前补模块/网络/命令行。
//!
//! 证据面受权限决定：本机不是管理员 ⇒ 用不了内核 ETW（进程/文件/镜像），
//! 只能拿 `Get-Process` / `Get-CimInstance` / `Get-NetTCPConnection` 这一层。
//! 「谁读了哪个文件」这类事件**采不到**，报告里写"未采集"，不编。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::event::{Event, now_ms};

pub struct Options {
    pub watch: Vec<PathBuf>,
    pub interval_ms: u64,
    pub capture_output: bool,
    /// 最多跟踪多少个进程（防止 `Get-Process` 采样把整机塞爆）。
    pub max_processes: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            watch: Vec::new(),
            interval_ms: 400,
            capture_output: true,
            max_processes: 512,
        }
    }
}

fn powershell(script: &str) -> Result<String> {
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .context("启动 powershell 失败")?;
    if !output.status.success() {
        bail!(
            "powershell 退出 {:?}：{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// pid → (ppid, name, command_line)
fn process_meta() -> Result<BTreeMap<u32, (u32, String, String)>> {
    let script = r#"
$ProgressPreference='SilentlyContinue'
Get-CimInstance Win32_Process | ForEach-Object {
  "{0}`t{1}`t{2}`t{3}" -f $_.ProcessId, $_.ParentProcessId, $_.Name, ($_.CommandLine -replace "`t", " ")
}
"#;
    let text = powershell(script)?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() < 4 {
            continue;
        }
        let Ok(pid) = parts[0].trim().parse::<u32>() else {
            continue;
        };
        let ppid = parts[1].trim().parse::<u32>().unwrap_or(0);
        map.insert(
            pid,
            (ppid, parts[2].trim().to_string(), parts[3].to_string()),
        );
    }
    Ok(map)
}

fn modules(pids: &[u32]) -> Result<Vec<(u32, String, String)>> {
    if pids.is_empty() {
        return Ok(Vec::new());
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        r#"
$ProgressPreference='SilentlyContinue'
foreach ($procId in @({list})) {{
  try {{
    (Get-Process -Id $procId -ErrorAction Stop).Modules | ForEach-Object {{
      "{{0}}`t{{1}}`t{{2}}" -f $procId, $_.ModuleName, $_.FileName
    }}
  }} catch {{ }}
}}
"#
    );
    let text = powershell(&script)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(3, '\t').collect();
        if parts.len() == 3
            && let Ok(pid) = parts[0].trim().parse::<u32>()
        {
            out.push((
                pid,
                parts[1].trim().to_string(),
                parts[2].trim().to_string(),
            ));
        }
    }
    Ok(out)
}

fn connections() -> Result<Vec<(u32, String, String)>> {
    let script = r#"
$ProgressPreference='SilentlyContinue'
Get-NetTCPConnection -ErrorAction SilentlyContinue | ForEach-Object {
  "{0}`t{1}`t{2}`t{3}" -f $_.OwningProcess, $_.RemoteAddress, $_.RemotePort, $_.State
}
"#;
    let text = powershell(script)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(4, '\t').collect();
        if parts.len() == 4
            && let Ok(pid) = parts[0].trim().parse::<u32>()
        {
            out.push((
                pid,
                format!("{}:{}", parts[1].trim(), parts[2].trim()),
                parts[3].trim().to_string(),
            ));
        }
    }
    Ok(out)
}

/// 路径 → (mtime_ns, 字节数)
type Snapshot = BTreeMap<PathBuf, (u64, u64)>;

fn snapshot_tree(root: &Path, limit: usize) -> BTreeMap<PathBuf, (u64, u64)> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u64, u64)>, limit: usize) {
        if out.len() >= limit {
            return;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if out.len() >= limit {
                return;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                walk(&path, out, limit);
            } else {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as u64)
                    .unwrap_or(0);
                out.insert(path, (mtime, meta.len()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out, limit);
    out
}

fn is_windows() -> bool {
    std::env::consts::OS == "windows"
}

/// 跑目标并采集；返回事件流。
pub fn run(target: &[String], options: &Options) -> Result<Vec<Event>> {
    if target.is_empty() {
        bail!("需要一个要观察的命令");
    }
    if !is_windows() {
        bail!("采集器目前只支持 Windows（其它平台只能对已有 JSONL 做 analyze）");
    }

    let mut events = Vec::new();
    let started = Instant::now();
    events.push(Event::Note {
        ts_ms: now_ms(),
        message:
            "采集面：进程树/模块/TCP/目录快照（非管理员，内核 ETW 不可用；文件读取事件未采集）"
                .to_string(),
    });

    let mut watchers: Vec<(PathBuf, Snapshot)> = Vec::new();
    for dir in &options.watch {
        watchers.push((dir.clone(), snapshot_tree(dir, 20_000)));
    }

    // 起目标
    let mut command = Command::new(&target[0]);
    command.args(&target[1..]);
    if options.capture_output {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("启动 {} 失败", target[0]))?;
    let root_pid = child.id();

    // 目标输出：两条管道各起一个读线程，采样循环不阻塞
    let captured: std::sync::Arc<std::sync::Mutex<Vec<(String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    if options.capture_output {
        let mut pipes: Vec<(&str, Box<dyn std::io::Read + Send>)> = Vec::new();
        if let Some(pipe) = child.stdout.take() {
            pipes.push(("stdout", Box::new(pipe)));
        }
        if let Some(pipe) = child.stderr.take() {
            pipes.push(("stderr", Box::new(pipe)));
        }
        for (stream, pipe) in pipes {
            let buffer = captured.clone();
            thread::spawn(move || {
                for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                    if let Ok(mut guard) = buffer.lock() {
                        guard.push((stream.to_string(), line));
                    }
                }
            });
        }
    }
    // 采样循环：Get-Process 比 CIM 轻，每轮一次
    let mut first_seen: BTreeMap<u32, (u64, String, u64, u64)> = BTreeMap::new();
    let deadline_poll = Duration::from_millis(options.interval_ms.max(100));
    // 中途补一次模块与命令行：短命进程在结束时已经消失，事后采不到。
    let mut mid_modules: Vec<(u32, String, String)> = Vec::new();
    let mut mid_meta: BTreeMap<u32, (u32, String, String)> = BTreeMap::new();
    let mut mid_taken = false;
    let mut ticks = 0u64;
    loop {
        let script = r#"
$ProgressPreference='SilentlyContinue'
@(Get-Process | ForEach-Object {
  "{0}`t{1}`t{2}`t{3}" -f $_.Id, $_.ProcessName, [int64]($_.WorkingSet64), [int64]($_.CPU * 1000)
}) | Out-String
"#;
        if let Ok(text) = powershell(script) {
            for line in text.lines() {
                let parts: Vec<&str> = line.splitn(4, '\t').collect();
                if parts.len() != 4 {
                    continue;
                }
                let Ok(pid) = parts[0].trim().parse::<u32>() else {
                    continue;
                };
                if first_seen.len() >= options.max_processes && !first_seen.contains_key(&pid) {
                    continue;
                }
                first_seen.entry(pid).or_insert_with(|| {
                    (
                        now_ms(),
                        parts[1].trim().to_string(),
                        parts[2].trim().parse().unwrap_or(0),
                        parts[3].trim().parse().unwrap_or(0),
                    )
                });
            }
        }
        // 目录快照差异
        for (dir, snapshot) in watchers.iter_mut() {
            let current = snapshot_tree(dir, 20_000);
            for (path, (mtime, size)) in &current {
                match snapshot.get(path) {
                    Some((old_mtime, _)) if old_mtime == mtime => {}
                    _ => events.push(Event::FileWrite {
                        ts_ms: now_ms(),
                        path: path.display().to_string(),
                        bytes: *size,
                    }),
                }
            }
            *snapshot = current;
        }
        ticks += 1;
        if !mid_taken && ticks >= 2 {
            mid_taken = true;
            mid_meta = process_meta().unwrap_or_default();
            mid_modules = modules(&[root_pid]).unwrap_or_default();
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if started.elapsed() > Duration::from_secs(60 * 30) {
            events.push(Event::Note {
                ts_ms: now_ms(),
                message: "超过 30 分钟上限，停止采样（目标仍在跑，已尝试终止）".to_string(),
            });
            let _ = child.kill();
            break;
        }
        thread::sleep(deadline_poll);
    }
    let _ = child.wait();

    // 结束：补命令行、模块、网络
    let mut after_meta = process_meta().unwrap_or_default();
    for (pid, info) in &mid_meta {
        after_meta.entry(*pid).or_insert_with(|| info.clone());
    }
    let mut tree: BTreeSet<u32> = BTreeSet::new();
    tree.insert(root_pid);
    let mut changed = true;
    while changed {
        changed = false;
        for (pid, (ppid, _, _)) in &after_meta {
            if tree.contains(ppid) && !tree.contains(pid) {
                tree.insert(*pid);
                changed = true;
            }
        }
    }
    for pid in &tree {
        let (ts_ms, name, working_set, cpu_ms) =
            first_seen
                .get(pid)
                .cloned()
                .unwrap_or((now_ms(), String::new(), 0, 0));
        let (ppid, cim_name, command_line) =
            after_meta
                .get(pid)
                .cloned()
                .unwrap_or((0, name.clone(), String::new()));
        events.push(Event::Process {
            ts_ms,
            pid: *pid,
            ppid,
            name: if cim_name.is_empty() { name } else { cim_name },
            command_line,
            working_set,
            cpu_ms,
        });
    }
    let tracked: Vec<u32> = tree.iter().copied().collect();
    let mut module_rows = mid_modules;
    module_rows.extend(modules(&tracked).unwrap_or_default());
    let mut seen_modules: BTreeSet<(u32, String)> = BTreeSet::new();
    for (pid, _module, path) in module_rows {
        if !seen_modules.insert((pid, path.clone())) {
            continue;
        }
        events.push(Event::Module {
            ts_ms: now_ms(),
            pid,
            process: after_meta
                .get(&pid)
                .map(|(_, name, _)| name.clone())
                .unwrap_or_default(),
            path,
            exports: None,
        });
    }
    for (pid, remote, state) in connections().unwrap_or_default() {
        if tree.contains(&pid) {
            events.push(Event::Connection {
                ts_ms: now_ms(),
                pid,
                remote,
                state,
            });
        }
    }
    if let Ok(mut guard) = captured.lock() {
        for (stream, line) in guard.drain(..) {
            events.push(Event::Output {
                ts_ms: now_ms(),
                stream,
                line,
            });
        }
    }
    events.push(Event::Note {
        ts_ms: now_ms(),
        message: format!(
            "采集结束：跟踪 {} 个进程，耗时 {} ms",
            tree.len(),
            started.elapsed().as_millis()
        ),
    });
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_detects_new_and_changed_files() {
        let dir = std::env::temp_dir().join(format!(
            "trit-procscan-snap-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        fs::write(&file, b"one").unwrap();
        let first = snapshot_tree(&dir, 100);
        assert_eq!(first.len(), 1);

        fs::write(&file, b"two-longer").unwrap();
        let second = snapshot_tree(&dir, 100);
        let (mtime, size) = second.get(&file).copied().unwrap();
        assert_eq!(size, 10, "长度变化必须被看到");
        assert_ne!(first.get(&file).copied(), Some((mtime, size)));

        fs::write(dir.join("b.txt"), b"x").unwrap();
        let third = snapshot_tree(&dir, 100);
        assert_eq!(third.len(), 2, "新文件必须被看到");
        let _ = fs::remove_dir_all(&dir);
    }
}
