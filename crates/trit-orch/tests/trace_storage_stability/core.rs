//! 轨迹存储稳定组：崩溃恢复（fail-closed + 行号）、并发追加、store/log 一致性、
//! 重放确定性、事件流与旧步骤流两种表示的交叉验证。

use trit_orch::{
    AppendOutcome, Baseline, Error, TraceEvent, TraceEventKind, TraceEventStore, TraceInvariant,
    TraceLog, TraceStep, check_trace, events_from_step, replay,
};

#[path = "../common/mod.rs"]
mod common;

use common::{inject, step, temp_path, valid_events};

// ---------------------------------------------------------------------------
// 崩溃恢复：文件截断在记录中间 / 坏 JSONL 行 / 空文件 / 半行尾巴
// ---------------------------------------------------------------------------

fn seeded_store_file(tag: &str, count: u64) -> std::path::PathBuf {
    let path = temp_path(tag);
    let store = TraceEventStore::new(&path);
    for seq in 1..=count {
        let event = TraceEvent::new("crash", seq, TraceEventKind::TaskSelected, "c", 100 + seq);
        store.append(&event).unwrap();
    }
    path
}

#[test]
fn crash_truncated_mid_record_fails_closed_with_line_number() {
    let path = seeded_store_file("trunc-mid", 3);
    let text = std::fs::read_to_string(&path).unwrap();
    let line_ends: Vec<usize> = text
        .match_indices('\n')
        .map(|(index, _)| index + 1)
        .collect();
    assert_eq!(line_ends.len(), 3, "夹具假设：三行三换行");

    // 在第 2 行记录中间截断（模拟进程在第 2 次 append 写到一半时崩溃）。
    let cut = line_ends[1] - 25;
    std::fs::write(&path, &text[..cut]).unwrap();
    let error = TraceEventStore::new(&path)
        .load()
        .expect_err("截断的记录必须 fail-closed");
    match error {
        Error::TraceParse { line, .. } => assert_eq!(line, 2, "报文要说清第几行"),
        other => panic!("expected trace_parse, got {}", other.kind()),
    }
    let _ = std::fs::remove_file(&path);
}

#[test]
fn crash_half_written_tail_and_bad_middle_line_fail_closed() {
    // 半行尾巴（第 4 次 append 只写了开头几个字节）。
    let tail_path = seeded_store_file("trunc-tail", 3);
    let text = std::fs::read_to_string(&tail_path).unwrap();
    std::fs::write(&tail_path, format!("{text}{{\"schema\"")).unwrap();
    match TraceEventStore::new(&tail_path).load() {
        Err(Error::TraceParse { line: 4, .. }) => {}
        other => panic!("半行尾巴必须报第 4 行，得到 {other:?}"),
    }
    let _ = std::fs::remove_file(&tail_path);

    // 中间坏行：第一条坏行胜出，行号是 1-based。
    let mid_path = seeded_store_file("bad-mid", 3);
    let text = std::fs::read_to_string(&mid_path).unwrap();
    let end_of_first = text.find('\n').unwrap() + 1;
    std::fs::write(
        &mid_path,
        format!(
            "{}{{oops}}\n{}",
            &text[..end_of_first],
            &text[end_of_first..]
        ),
    )
    .unwrap();
    match TraceEventStore::new(&mid_path).load() {
        Err(Error::TraceParse { line: 2, .. }) => {}
        other => panic!("中间坏行必须报第 2 行，得到 {other:?}"),
    }
    let _ = std::fs::remove_file(&mid_path);
}

#[test]
fn tail_truncation_is_detected_by_watermark() {
    let path = seeded_store_file("trunc-watermark", 3);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "夹具假设：三条事件");
    lines.pop();
    std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

    // 水位记录着最后一次成功 append 的 seq；删掉尾部事件后 max_seq < watermark
    // 必须 fail-closed，而不是把“文件短了”当成“事件还没发生”。
    let error = TraceEventStore::new(&path)
        .load()
        .expect_err("尾部截断必须 fail-closed");
    assert_eq!(error.kind(), "trace_truncated");
    let _ = std::fs::remove_file(&path);
    let mut watermark = path.as_os_str().to_os_string();
    watermark.push(".watermark");
    let _ = std::fs::remove_file(watermark);
}

#[test]
fn empty_and_whitespace_files_load_as_empty_logs() {
    let path = temp_path("empty-file");
    std::fs::write(&path, "").unwrap();
    assert!(TraceEventStore::new(&path).load().unwrap().is_empty());

    std::fs::write(&path, "   \n\t\n").unwrap();
    assert!(
        TraceEventStore::new(&path).load().unwrap().is_empty(),
        "纯空白文件也是空日志"
    );
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// 并发追加、AppendOutcome 语义
// ---------------------------------------------------------------------------

#[test]
fn concurrent_append_step_keeps_every_line_intact_and_log_clean() {
    let path = temp_path("concurrent");
    let store = std::sync::Arc::new(TraceEventStore::new(&path));
    let threads = 8u64;
    let steps_per_thread = 12u64;

    std::thread::scope(|scope| {
        for thread in 0..threads {
            let store = std::sync::Arc::clone(&store);
            scope.spawn(move || {
                for step_index in 0..steps_per_thread {
                    // 全局稠密编号：第 (thread, step_index) 步占 4 个连续 seq。
                    let ordinal = thread * steps_per_thread + step_index;
                    let base = 1 + ordinal * 4;
                    let revision = 1 + ordinal;
                    let ids: Vec<String> =
                        (0..2).map(|index| format!("t{thread}-{index}")).collect();
                    let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                    store
                        .append_step("cc", base, &step(revision, &ids), 1_000 + ordinal)
                        .expect("并发 append_step 不能失败");
                }
            });
        }
    });

    let events = store.load().expect("任何一行都不允许交错损坏");
    assert_eq!(events.len(), (threads * steps_per_thread * 4) as usize);
    let log = store.load_log("cc").expect("重建日志");
    assert_eq!(log.len(), events.len(), "同 trace 全部收进来");
    let seqs: Vec<u64> = log.events().map(|event| event.seq).collect();
    assert_eq!(
        seqs,
        (1..=threads * steps_per_thread * 4).collect::<Vec<_>>(),
        "并发乱序落盘后必须按 seq 完整归位"
    );
    let report = check_trace(&log);
    assert!(
        report.is_clean(),
        "并发产物必须仍然干净：{:?}",
        report.violations
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn append_outcome_semantics_survive_concurrent_rebuild() {
    let path = temp_path("outcome");
    let store = TraceEventStore::new(&path);
    // 并发写入 8 个线程 × 2 步，线程间 seq 交错。
    std::thread::scope(|scope| {
        for thread in 0..8u64 {
            let store = &store;
            scope.spawn(move || {
                for step_index in 0..2u64 {
                    let ordinal = thread * 2 + step_index;
                    store
                        .append_step("oc", 1 + ordinal * 4, &step(1 + ordinal, &["a"]), 900)
                        .unwrap();
                }
            });
        }
    });
    let mut log = store.load_log("oc").expect("重建");
    assert_eq!(log.len(), 64);

    // AppendOutcome 语义在并发产物上必须原样成立：
    // 同 id 重放 → Duplicate；同 seq 不同 id → 拒绝；迟到 seq → 标记迟到。
    let existing = log.events().next().unwrap().clone();
    let mut clashing = TraceEvent::new("oc", existing.seq, TraceEventKind::EffectSent, "c", 1);
    clashing.id = "oc:clash".into();
    let late = TraceEvent::new(
        "oc",
        existing.seq.saturating_sub(1),
        TraceEventKind::TaskSelected,
        "c",
        1,
    );

    assert_eq!(log.append(existing).unwrap(), AppendOutcome::Duplicate);
    assert!(matches!(
        log.append(clashing),
        Err(Error::TraceSeqConflict { .. })
    ));
    assert_eq!(log.append(late).unwrap(), AppendOutcome::LateButRecorded);
    let _ = std::fs::remove_file(&path);
}

// ---------------------------------------------------------------------------
// store/log 一致性
// ---------------------------------------------------------------------------

#[test]
fn load_log_equals_in_memory_log_with_same_check_result() {
    let path = temp_path("consistency");
    let events = valid_events(31, "t", 120);
    let mut sealed = Vec::with_capacity(events.len());
    let mut previous = String::new();
    for (index, event) in events.iter().enumerate() {
        let mut copy = event.clone();
        copy.chain_seq = (index + 1) as u64;
        copy.seal(&previous);
        previous = copy.payload_hash.clone();
        sealed.push(copy);
    }
    let mut memory = TraceLog::new("t");
    let store = TraceEventStore::new(&path);
    for (event, sealed_event) in events.iter().zip(&sealed) {
        store.append(event).unwrap();
        memory.append(sealed_event.clone()).unwrap();
    }
    let loaded = store.load_log("t").unwrap();
    assert_eq!(loaded, memory, "store/log 一致性：逐事件相等");
    assert_eq!(
        check_trace(&loaded),
        check_trace(&memory),
        "两份表示的 check 结果必须一致"
    );

    // 换个视角：注入违反后，从盘上重建的日志与内存日志报出同样的证书。
    let bad_path = temp_path("consistency-bad");
    let mut injected = valid_events(32, "t", 120);
    inject(&mut injected, TraceInvariant::HashChainLinked, 32);
    let mut memory_bad = TraceLog::new("t");
    for event in &injected {
        memory_bad.append(event.clone()).unwrap();
    }
    assert!(
        !check_trace(&memory_bad).is_clean(),
        "内存表示必须报出注入的违反"
    );

    // 落盘层更强：内容与摘要不符时 load 直接拒绝，而不是把篡改当证据。
    let bad_store = TraceEventStore::new(&bad_path);
    for event in &valid_events(33, "t", 3) {
        bad_store.append(event).unwrap();
    }
    let text = std::fs::read_to_string(&bad_path).unwrap();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    lines[1] = lines[1].replacen("\"observed_at\":", "\"observed_at\":9", 1);
    std::fs::write(&bad_path, lines.join("\n") + "\n").unwrap();
    assert!(matches!(
        bad_store.load(),
        Err(Error::TraceHashMismatch { line: 2, .. })
    ));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&bad_path);
}

// ---------------------------------------------------------------------------
// 重放确定性与双表示交叉验证
// ---------------------------------------------------------------------------

/// 只比较内容：去掉落盘层分配的写入序与摘要。
fn content_only(mut events: Vec<TraceEvent>) -> Vec<TraceEvent> {
    for event in &mut events {
        event.chain_seq = 0;
        event.payload_hash.clear();
        event.prev_hash.clear();
    }
    events
}

/// 旧格式迁移：每步占 4 条连续 seq 的事件，按块折回 `TraceStep`。
fn fold_blocks(store: &TraceEventStore) -> Vec<TraceStep> {
    store
        .load()
        .expect("load")
        .chunks(4)
        .map(|chunk| {
            let mut block = TraceLog::new("t");
            for event in chunk {
                block.append(event.clone()).unwrap();
            }
            block.latest_step().expect("四条完整状态事件必须能折回一步")
        })
        .collect()
}

#[test]
fn replay_of_store_steps_is_deterministic_field_by_field() {
    let path = temp_path("replay-det");
    let store = TraceEventStore::new(&path);
    let steps = vec![
        step(1, &["a"]),
        step(2, &["a", "b"]),
        step(3, &["a", "b", "c"]),
    ];
    for (index, step_value) in steps.iter().enumerate() {
        store
            .append_step("t", 1 + (index as u64) * 4, step_value, 500 + index as u64)
            .unwrap();
    }

    let first_steps = fold_blocks(&store);
    let second_steps = fold_blocks(&store);
    assert_eq!(first_steps, second_steps, "同 store 两次读回必须逐字段相等");
    assert_eq!(first_steps, steps, "折回的步骤要等于写进去的步骤");

    let report_a = replay(&Baseline, &first_steps).unwrap();
    let report_b = replay(&Baseline, &second_steps).unwrap();
    assert_eq!(report_a, report_b, "两次 replay 必须逐字段相等");
    assert_eq!(report_a.steps, 3);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn event_store_and_step_store_agree_on_the_same_stream() {
    // 同一串 TraceStep 两种表示：shadow::TraceStore（一行一步）与
    // TraceEventStore（一行一事件）必须折回同一批步骤、得到同一份 replay 报告。
    let steps = vec![step(4, &["a"]), step(5, &["a", "b"])];

    let events_path = temp_path("cross-events");
    let event_store = TraceEventStore::new(&events_path);
    for (index, step_value) in steps.iter().enumerate() {
        event_store
            .append_step("t", 1 + (index as u64) * 4, step_value, 42)
            .unwrap();
    }

    let steps_path = temp_path("cross-steps");
    let step_store = trit_orch::TraceStore::new(&steps_path);
    for step_value in &steps {
        step_store.append(step_value).unwrap();
    }

    let from_events = fold_blocks(&event_store);
    let from_steps = step_store.load().unwrap();
    assert_eq!(from_events, from_steps, "两种表示折回同一批步骤");
    assert_eq!(
        content_only(events_from_step("t", 1, &steps[0], 42)),
        content_only(event_store.load().unwrap()[..4].to_vec()),
        "迁移函数产出与 store 落盘内容一致"
    );
    assert_eq!(
        replay(&Baseline, &from_events).unwrap(),
        replay(&Baseline, &from_steps).unwrap()
    );
    let _ = std::fs::remove_file(&events_path);
    let _ = std::fs::remove_file(&steps_path);
}
