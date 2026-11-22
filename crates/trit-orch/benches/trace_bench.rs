//! 轨迹域基准：`harness = false`，零第三方依赖（只用 std + 本 crate）。
//!
//! 四项读数，每项先丢一轮预热再测 7 轮取**中位**（防缓存/调度毛刺把单轮读数当结论）：
//! 1. `check_trace` 在 10 万事件日志上的耗时；
//! 2. `TraceEventStore::append_step` 落盘吞吐（每事件一次 fsync，读数含同步代价）；
//! 3. `load_log` 大文件逐行解析 + 重建日志；
//! 4. 崩溃恢复：截断文件 load 拒绝（fail-closed 路径的代价）。
//!
//! 与外部系统的可比性在汇报里单独说明：OpenTelemetry 的 span 完整性（trace/span/parent
//! 引用闭合）没有官方吞吐声明，可比的是**结构语义**而非数字。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use trit_orch::{
    Ledger, Limits, Resources, Snapshot, TraceEvent, TraceEventKind, TraceEventStore, TraceLog,
    TracePayload, check_trace,
};

const EVENTS: u64 = 100_000;
const STEPS_PER_ROUND: u64 = 125; // 125 步 × 4 事件 = 500 次 fsync 落盘/轮
const ROUNDS: usize = 7;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

/// 10 万事件日志：每 10 条一组 plan(rev)/snapshot(rev) 对齐对，其余轻事件。
fn bench_log(trace: &str, events: u64) -> TraceLog {
    let mut log = TraceLog::new(trace);
    for seq in 1..=events {
        let observed_at = 1_000_000 + seq;
        let event = if seq % 10 == 1 {
            TraceEvent::new(
                trace,
                seq,
                TraceEventKind::PlanPatched,
                "bench",
                observed_at,
            )
            .with_payload(TracePayload::Plan {
                revision: seq / 10,
                tasks: Vec::new(),
            })
        } else if seq % 10 == 2 {
            TraceEvent::new(
                trace,
                seq,
                TraceEventKind::SnapshotCaptured,
                "bench",
                observed_at,
            )
            .with_payload(TracePayload::Snapshot {
                snapshot: Snapshot {
                    revision: seq / 10,
                    observations: BTreeMap::new(),
                },
            })
        } else {
            TraceEvent::new(
                trace,
                seq,
                TraceEventKind::TaskSelected,
                "bench",
                observed_at,
            )
        };
        log.append(event).expect("基准夹具事件必须合法");
    }
    log
}

/// 直接批量写 JSONL（不经 append 的逐次 fsync）：load 基准只关心文件内容。
fn write_bench_file(path: &Path, trace: &str, events: u64) {
    let mut file = fs::File::create(path).expect("create bench file");
    let mut buffer = String::new();
    for seq in 1..=events {
        let observed_at = 1_000_000 + seq;
        let event = if seq % 10 == 1 {
            TraceEvent::new(
                trace,
                seq,
                TraceEventKind::PlanPatched,
                "bench",
                observed_at,
            )
            .with_payload(TracePayload::Plan {
                revision: seq / 10,
                tasks: Vec::new(),
            })
        } else if seq % 10 == 2 {
            TraceEvent::new(
                trace,
                seq,
                TraceEventKind::SnapshotCaptured,
                "bench",
                observed_at,
            )
            .with_payload(TracePayload::Snapshot {
                snapshot: Snapshot {
                    revision: seq / 10,
                    observations: BTreeMap::new(),
                },
            })
        } else {
            TraceEvent::new(
                trace,
                seq,
                TraceEventKind::TaskSelected,
                "bench",
                observed_at,
            )
        };
        buffer.push_str(&serde_json::to_string(&event).expect("serialize"));
        buffer.push('\n');
        if buffer.len() > 1 << 20 {
            file.write_all(buffer.as_bytes()).expect("write");
            buffer.clear();
        }
    }
    file.write_all(buffer.as_bytes()).expect("write");
}

fn temp_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!("trit-orch-trace-bench-{}", std::process::id()));
    fs::create_dir_all(&path).expect("mkdir");
    path
}

fn main() {
    let dir = temp_dir();
    println!("== trit-orch 轨迹域基准 ==  每项 1 轮预热丢弃 + {ROUNDS} 轮取中位");

    // ① check()：10 万事件。
    let log = bench_log("bench", EVENTS);
    let mut samples = Vec::new();
    for round in 0..=ROUNDS {
        let started = Instant::now();
        let report = check_trace(&log);
        let elapsed = started.elapsed();
        assert!(
            report.is_clean(),
            "基准日志必须干净：{:?}",
            report.violations
        );
        if round > 0 {
            samples.push(elapsed);
        }
    }
    let check_median = median(samples);

    // ② append_step 吞吐：每事件一次原子写 + fsync。
    let append_path = dir.join("append.jsonl");
    let store = TraceEventStore::new(&append_path);
    let step = trit_orch::TraceStep {
        revision: 1,
        tasks: Vec::new(),
        snapshot: Snapshot {
            revision: 1,
            observations: BTreeMap::new(),
        },
        limits: Limits {
            workers: 1,
            capacity: Resources::new(1, 1),
            tokens: 1_000,
        },
        ledger: Ledger::default(),
    };
    let mut samples = Vec::new();
    for round in 0..=ROUNDS {
        let started = Instant::now();
        for ordinal in 0..STEPS_PER_ROUND {
            let base = 1 + round as u64 * STEPS_PER_ROUND * 4 + ordinal * 4;
            store
                .append_step("bench", base, &step, 1_000 + base)
                .expect("append_step");
        }
        if round > 0 {
            samples.push(started.elapsed());
        }
    }
    let append_median = median(samples);
    let append_events_per_sec = (STEPS_PER_ROUND * 4) as f64 / append_median.as_secs_f64();

    // ③ load_log：10 万事件文件解析 + 重建。
    let load_path = dir.join("load.jsonl");
    write_bench_file(&load_path, "bench", EVENTS);
    let load_store = TraceEventStore::new(&load_path);
    let file_bytes = fs::metadata(&load_path).expect("meta").len();
    let mut samples = Vec::new();
    for round in 0..=ROUNDS {
        let started = Instant::now();
        let loaded = load_store.load_log("bench").expect("load_log");
        if round > 0 {
            samples.push(started.elapsed());
        }
        assert_eq!(loaded.len() as u64, EVENTS);
    }
    let load_median = median(samples);

    // ④ 崩溃恢复：把 10 万事件的文件在末条记录中间截断，load 必须 fail-closed。
    //    拒绝路径要先解析完前 99_999 条好行，所以这就是"读到坏处为止"的真实代价。
    let crash_path = dir.join("crash.jsonl");
    write_bench_file(&crash_path, "bench", EVENTS);
    let text = fs::read_to_string(&crash_path).expect("read");
    fs::write(&crash_path, &text[..text.len() - 40]).expect("truncate");
    let crash_store = TraceEventStore::new(&crash_path);
    let mut samples = Vec::new();
    for round in 0..=ROUNDS {
        let started = Instant::now();
        let result = crash_store.load();
        if round > 0 {
            samples.push(started.elapsed());
        }
        assert!(result.is_err(), "截断文件必须被拒绝");
    }
    let crash_median = median(samples);

    let events = EVENTS.to_string();
    let events_per_round = (STEPS_PER_ROUND * 4).to_string();
    println!();
    println!("+----------------------------------------------+----------------+----------------+");
    println!("| metric                                       | input          | median         |");
    println!("+----------------------------------------------+----------------+----------------+");
    println!(
        "| check_trace (6 invariants)                   | {events:>8} evts | {:>14.2?} |",
        check_median
    );
    println!(
        "| append_step throughput (fsync per event)     | {events_per_round:>8} evts | {:>10.0} e/s  |",
        append_events_per_sec
    );
    println!(
        "| load_log parse + rebuild                     | {events:>8} evts | {:>14.2?} |",
        load_median
    );
    println!(
        "| crash recovery: truncated load rejected      | {events:>8} evts | {:>14.2?} |",
        crash_median
    );
    println!("+----------------------------------------------+----------------+----------------+");
    println!(
        "load file size: {file_bytes} bytes; per-event: check {:.1} ns, load_log {:.1} ns",
        check_median.as_nanos() as f64 / EVENTS as f64,
        load_median.as_nanos() as f64 / EVENTS as f64,
    );

    let _ = fs::remove_dir_all(&dir);
}
