//! 回执 id 冲突：同 key 不同 id 必须拒绝，而不是静默复用旧条目。

use trit_orch::{Error, ReceiptLedger};

#[test]
fn same_key_with_different_id_is_a_conflict() {
    let mut ledger = ReceiptLedger::new();
    let first = ledger.record("r1", "k").unwrap();
    assert_eq!(first.id, "r1");
    assert_eq!(ledger.record("r1", "k").unwrap(), first, "同 id 才是幂等");
    assert_eq!(
        ledger.record("r2", "k"),
        Err(Error::ReceiptIdConflict {
            key: "k".into(),
            existing: "r1".into(),
            incoming: "r2".into(),
        }),
        "ORCH_RECEIPT_ID_CONFLICT"
    );
}
