//! 双轨并行执行器：按策略把任务派给三进制轨 / 二进制轨，并用槽位真并行。
//!
//! 策略本身在 `schedule`（纯函数、可穷举单测）；这里只负责线程、队列与结果汇总。
//! 不抢占：已启动的外部命令不打断；"优先"只体现为"不派发"。

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::build_source;
use crate::schedule::{self, Prefer, Track};
use trit_core::Algebra;

// ---------- 双轨并行（--manifest） ----------

#[derive(Debug, Clone)]
pub(crate) struct JobSpec {
    name: String,
    track: Track,
    source: Option<PathBuf>,
    out: Option<PathBuf>,
    cwd: Option<PathBuf>,
    cmd: Option<Vec<String>>,
    output: Option<String>,
    expect: Option<String>,
}

pub(crate) fn parse_prefer(name: &str) -> Result<Prefer> {
    match name {
        "ternary" => Ok(Prefer::Ternary),
        "binary" => Ok(Prefer::Binary),
        "balanced" => Ok(Prefer::Balanced),
        other => bail!("未知的 --prefer {other:?}，可选：ternary / binary / balanced"),
    }
}

pub(crate) fn load_jobs(path: &Path) -> Result<Vec<JobSpec>> {
    let text =
        fs::read_to_string(path).with_context(|| format!("读清单 {} 失败", path.display()))?;
    let value: Value =
        serde_json::from_str(&text).with_context(|| format!("{} 不是合法 JSON", path.display()))?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let jobs = value
        .get("jobs")
        .and_then(Value::as_array)
        .context("清单缺少 jobs 数组")?;
    let mut out = Vec::with_capacity(jobs.len());
    for (index, job) in jobs.iter().enumerate() {
        let name = job
            .get("name")
            .and_then(Value::as_str)
            .with_context(|| format!("jobs[{index}] 缺少 name"))?
            .to_string();
        let kind = job
            .get("kind")
            .and_then(Value::as_str)
            .with_context(|| format!("jobs[{index}] 缺少 kind"))?;
        let rel = |key: &str| -> Option<PathBuf> {
            job.get(key).and_then(Value::as_str).map(|p| base.join(p))
        };
        let str_of = |key: &str| -> Option<String> {
            job.get(key).and_then(Value::as_str).map(str::to_string)
        };
        match kind {
            "ternary" | "verilog" => {
                let source =
                    rel("source").with_context(|| format!("{kind} 任务 {index} 缺 source"))?;
                out.push(JobSpec {
                    name,
                    track: Track::Ternary,
                    source: Some(source),
                    out: rel("out"),
                    cwd: None,
                    cmd: None,
                    output: str_of("output"),
                    expect: str_of("expect"),
                });
            }
            "binary" => {
                let cmd: Vec<String> = job
                    .get("cmd")
                    .and_then(Value::as_array)
                    .with_context(|| format!("binary 任务 {index} 缺 cmd 数组"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            .with_context(|| format!("binary 任务 {index} 的 cmd 含非字符串"))
                    })
                    .collect::<Result<_>>()?;
                if cmd.is_empty() {
                    bail!("binary 任务 {index} 的 cmd 为空");
                }
                out.push(JobSpec {
                    name,
                    track: Track::Binary,
                    source: None,
                    out: None,
                    cwd: rel("cwd"),
                    cmd: Some(cmd),
                    output: None,
                    expect: None,
                });
            }
            other => bail!("jobs[{index}] 未知 kind {other:?}（ternary / binary）"),
        }
    }
    Ok(out)
}

/// 干跑：只给分配方案（不执行）。balanced = 清单顺序，其余 = 主轨在前。
pub(crate) fn dry_run_plan(jobs: &[JobSpec], prefer: Prefer) -> Value {
    let order: Vec<usize> = if prefer == Prefer::Balanced {
        (0..jobs.len()).collect()
    } else {
        let (primary, secondary) = match prefer {
            Prefer::Binary => (Track::Binary, Track::Ternary),
            _ => (Track::Ternary, Track::Binary),
        };
        let mut out: Vec<usize> = (0..jobs.len())
            .filter(|&i| jobs[i].track == primary)
            .collect();
        out.extend((0..jobs.len()).filter(|&i| jobs[i].track == secondary));
        out
    };
    json!({
        "mode": "dry-run",
        "prefer": format!("{prefer:?}").to_lowercase(),
        "plan": order
            .iter()
            .map(|&i| json!({
                "name": jobs[i].name,
                "track": match jobs[i].track { Track::Ternary => "ternary", Track::Binary => "binary" },
                "owner": match jobs[i].track { Track::Ternary => "ternary-track", Track::Binary => "binary-track" },
            }))
            .collect::<Vec<_>>(),
    })
}

pub(crate) fn execute_job(job: &JobSpec, algebra: Algebra, out_root: &Path) -> Value {
    let started = Instant::now();
    let result = match job.track {
        Track::Ternary => {
            let source = job.source.as_deref().expect("ternary 任务必有 source");
            let out = job.out.clone().unwrap_or_else(|| out_root.join(&job.name));
            build_source(
                source,
                &out,
                algebra,
                false,
                job.output.as_deref(),
                job.expect.as_deref(),
            )
            .map(|summary| {
                json!({
                    "cached": summary.get("cached").cloned().unwrap_or(json!(false)),
                    "out": out.display().to_string(),
                })
            })
        }
        Track::Binary => {
            let cmd = job.cmd.as_ref().expect("binary 任务必有 cmd");
            let mut command = std::process::Command::new(&cmd[0]);
            command.args(&cmd[1..]);
            if let Some(cwd) = &job.cwd {
                command.current_dir(cwd);
            }
            // 捕获子进程输出，避免污染本工具的 JSON stdout；失败时带上尾部。
            command
                .output()
                .map_err(|error| anyhow::anyhow!("启动 {} 失败：{error}", cmd[0]))
                .and_then(|output| {
                    let status = output.status;
                    if status.success() {
                        return Ok(json!({"exit": status.code()}));
                    }
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let tail: String = stderr.chars().rev().take(400).collect::<String>();
                    let tail: String = tail.chars().rev().collect();
                    bail!("{} 失败（exit {:?}）：{}", cmd[0], status.code(), tail)
                })
        }
    };
    let ms = started.elapsed().as_millis() as u64;
    match result {
        Ok(extra) => {
            let mut value = json!({
                "name": job.name,
                "track": match job.track { Track::Ternary => "ternary", Track::Binary => "binary" },
                "ok": true,
                "ms": ms,
            });
            if let (Some(map), Some(extra_map)) = (value.as_object_mut(), extra.as_object()) {
                for (key, v) in extra_map {
                    map.insert(key.clone(), v.clone());
                }
            }
            value
        }
        Err(error) => json!({
            "name": job.name,
            "track": match job.track { Track::Ternary => "ternary", Track::Binary => "binary" },
            "ok": false,
            "ms": ms,
            "error": format!("{error:#}"),
        }),
    }
}

pub(crate) struct Shared {
    ternary: VecDeque<usize>,
    binary: VecDeque<usize>,
    fifo: VecDeque<usize>,
    running_ternary: usize,
    running_binary: usize,
    results: Vec<Option<Value>>,
    remaining: usize,
    prefer: Prefer,
    slots: usize,
}

pub(crate) fn take_next(shared: &mut Shared) -> Option<usize> {
    if shared.prefer == Prefer::Balanced {
        let index = shared.fifo.pop_front()?;
        return Some(index);
    }
    let load = schedule::Load {
        ternary_pending: shared.ternary.len(),
        binary_pending: shared.binary.len(),
        ternary_running: shared.running_ternary,
        binary_running: shared.running_binary,
    };
    match schedule::pick(load, shared.prefer, shared.slots)? {
        Track::Ternary => {
            let index = shared.ternary.pop_front()?;
            shared.running_ternary += 1;
            Some(index)
        }
        Track::Binary => {
            let index = shared.binary.pop_front()?;
            shared.running_binary += 1;
            Some(index)
        }
    }
}

pub(crate) fn run_parallel(
    manifest: &Path,
    out_root: &Path,
    algebra: Algebra,
    slots: usize,
    prefer: Prefer,
    dry_run: bool,
) -> Result<()> {
    let jobs = load_jobs(manifest)?;
    if jobs.is_empty() {
        bail!("清单里没有任务");
    }
    if dry_run {
        println!("{}", dry_run_plan(&jobs, prefer));
        return Ok(());
    }
    let slots = slots.max(1).min(jobs.len());
    let shared = Mutex::new(Shared {
        ternary: jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.track == Track::Ternary)
            .map(|(i, _)| i)
            .collect(),
        binary: jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| j.track == Track::Binary)
            .map(|(i, _)| i)
            .collect(),
        fifo: (0..jobs.len()).collect(),
        running_ternary: 0,
        running_binary: 0,
        results: vec![None; jobs.len()],
        remaining: jobs.len(),
        prefer,
        slots,
    });
    let cv = Condvar::new();
    let wall_start = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..slots {
            scope.spawn(|| {
                loop {
                    let index = {
                        let mut guard = shared.lock().expect("调度锁中毒");
                        loop {
                            if let Some(index) = take_next(&mut guard) {
                                break index;
                            }
                            if guard.remaining == 0 {
                                cv.notify_all();
                                return;
                            }
                            guard = cv.wait(guard).expect("调度等待中毒");
                        }
                    };
                    let track = jobs[index].track;
                    let value = execute_job(&jobs[index], algebra, out_root);
                    let mut guard = shared.lock().expect("调度锁中毒");
                    guard.results[index] = Some(value);
                    guard.remaining -= 1;
                    match track {
                        Track::Ternary => guard.running_ternary -= 1,
                        Track::Binary => guard.running_binary -= 1,
                    }
                    cv.notify_all();
                }
            });
        }
    });
    let wall_ms = wall_start.elapsed().as_millis() as u64;
    let mut guard = shared.lock().expect("调度锁中毒");
    let results: Vec<Value> = guard
        .results
        .drain(..)
        .map(|v| v.unwrap_or_else(|| json!({"ok": false, "error": "任务未执行"})))
        .collect();
    drop(guard);
    let failed = results.iter().filter(|v| v["ok"] != json!(true)).count();
    let busy_ms: u64 = results.iter().filter_map(|v| v["ms"].as_u64()).sum();
    let speedup = if wall_ms == 0 {
        json!(null)
    } else {
        json!((busy_ms as f64 / wall_ms as f64 * 100.0).round() / 100.0)
    };
    let summary = json!({
        "mode": "parallel",
        "prefer": format!("{prefer:?}").to_lowercase(),
        "slots": slots,
        "jobs": results,
        "failed": failed,
        "wall_ms": wall_ms,
        "busy_ms": busy_ms,
        "speedup_vs_serial": speedup,
        "note": "speedup = busy_ms / wall_ms；仅统计本工具看到的耗时，外部命令的编译缓存不计",
    });
    println!("{summary}");
    if failed > 0 {
        bail!("{failed} 个任务失败");
    }
    Ok(())
}
