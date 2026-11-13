use trit_orch::{Error, Hlc, TraceEvent, TraceEventKind, TraceLog, merge_logs};

fn event(trace_id: &str, seq: u64, id: &str, hlc: Option<Hlc>) -> TraceEvent {
    let event = TraceEvent::new(trace_id, seq, TraceEventKind::TaskSelected, "c", 100).with_id(id);
    match hlc {
        Some(hlc) => event.with_hlc(hlc),
        None => event,
    }
}

fn ids(events: &[TraceEvent]) -> Vec<&str> {
    events.iter().map(|event| event.id.as_str()).collect()
}

#[test]
fn hlc_orders_by_physical_counter_writer() {
    assert!(Hlc::new(10, 0, 1) < Hlc::new(10, 0, 2));
    assert!(Hlc::new(10, 1, 0) > Hlc::new(10, 0, 9));
    assert!(Hlc::new(11, 0, 0) > Hlc::new(10, u32::MAX, 999));
    assert_eq!(Hlc::ZERO, Hlc::default());
}

#[test]
fn merge_orders_concurrent_events_by_hlc() {
    let mut left = TraceLog::new("a");
    let mut right = TraceLog::new("b");
    left.append(event("a", 1, "a1", Some(Hlc::new(10, 0, 2))))
        .unwrap();
    right
        .append(event("b", 1, "b1", Some(Hlc::new(10, 0, 1))))
        .unwrap();

    let merged = merge_logs(&[&left, &right]).unwrap();
    assert_eq!(ids(&merged), ["b1", "a1"]);
}

#[test]
fn merge_preserves_per_trace_seq_even_if_hlc_is_reversed() {
    let mut log = TraceLog::new("t");
    log.append(event("t", 1, "t1", Some(Hlc::new(20, 0, 1))))
        .unwrap();
    log.append(event("t", 2, "t2", Some(Hlc::new(10, 0, 1))))
        .unwrap();

    let merged = merge_logs(&[&log]).unwrap();
    assert_eq!(ids(&merged), ["t1", "t2"], "seq 因果不能被 HLC 覆盖");
}

#[test]
fn merge_deduplicates_replicated_events_and_rejects_conflicts() {
    let replicated = event("t", 1, "e1", Some(Hlc::new(10, 0, 1)));
    let mut first = TraceLog::new("t");
    let mut second = TraceLog::new("t");
    first.append(replicated.clone()).unwrap();
    second.append(replicated).unwrap();

    let merged = merge_logs(&[&first, &second]).unwrap();
    assert_eq!(merged.len(), 1, "同一事件重复出现只保留一份");

    let mut conflict = TraceLog::new("t");
    conflict
        .append(TraceEvent::new("t", 1, TraceEventKind::TaskHeld, "c", 100).with_id("e1"))
        .unwrap();
    assert!(matches!(
        merge_logs(&[&first, &conflict]),
        Err(Error::TraceEventConflict { .. })
    ));
}

#[test]
fn merge_fails_closed_on_causality_cycles() {
    let mut log = TraceLog::new("t");
    log.append(event("t", 1, "a", Some(Hlc::new(10, 0, 1))).with_causation_id("b"))
        .unwrap();
    log.append(event("t", 2, "b", Some(Hlc::new(11, 0, 1))).with_causation_id("a"))
        .unwrap();

    assert!(matches!(
        merge_logs(&[&log]),
        Err(Error::TraceCausalityCycle { .. })
    ));
}
