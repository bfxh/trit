//! TraceLog 尾部水位：声明 tail 后 gaps 能报告尾部缺口。

use trit_orch::{TraceEvent, TraceEventKind, TraceLog, check_trace};

#[test]
fn tail_watermark_reports_trailing_gap() {
    let mut log = TraceLog::new("t");
    log.append(TraceEvent::new(
        "t",
        1,
        TraceEventKind::TaskSelected,
        "c",
        10,
    ))
    .unwrap();
    assert!(log.gaps().is_empty(), "没有 tail 时不猜末尾");

    let log = log.with_tail(3);
    assert_eq!(log.gaps(), vec![(2, 3)]);
    assert!(
        !check_trace(&log).is_clean(),
        "尾部缺口必须被 seq_contiguous 报出来"
    );
}
