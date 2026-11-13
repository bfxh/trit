//! 审计历史摘要的测试。

use trit_wspace::audit::AuditLog;
use trit_wspace::history::summary;
use trit_wspace::{Edit, Layer, Net, Patch, Trit};

fn tiny_net() -> Net {
    Net::new(vec![Layer::new(1, 2, &[1, -1], &[0]).expect("l")]).expect("net")
}

#[test]
fn summarizes_applied_and_skipped() {
    let net = tiny_net();
    let patch = Patch::new(vec![Edit::SetTrit {
        layer: 0,
        row: 0,
        col: 0,
        to: Trit::Neg,
    }]);
    let mut after = net.clone();
    patch.apply(&mut after).expect("apply");
    let mut log = AuditLog::new();
    log.record("first", &net, &after, patch.clone(), true, false)
        .expect("record");
    log.record("skipped", &after, &after, patch, false, false)
        .expect("record");
    let summary = summary(&log);
    assert_eq!(summary.entries, 2);
    assert_eq!(summary.applied, 1);
    assert_eq!(summary.skipped, 1);
    assert_eq!(summary.edits, 2);
    let timestamps = trit_wspace::history_time::extract(&log.to_jsonl());
    assert_eq!(timestamps.len(), 2);
    assert!(timestamps.iter().all(Option::is_some));
}
