//! Time-indexed point-in-time recovery.

use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tritdb_testkit::TempDir;
use tritdb_txn::{TxnError, TxnStore};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64
}

#[test]
fn recover_at_time_uses_the_indexed_commit_lsn() {
    let dir = TempDir::new("txn-pitr-time");
    let wal = dir.join("time.wal");
    let store = TxnStore::open(&wal).expect("open");

    let mut stamps = Vec::new();
    for i in 0..6 {
        let mut txn = store.begin().expect("begin");
        txn.put(format!("k{i}").as_bytes(), b"v1").expect("put");
        txn.commit().expect("commit");
        stamps.push(now_ms());
        // Keep successive commits in distinct milliseconds so the lookup
        // target is unambiguous.
        thread::sleep(Duration::from_millis(10));
    }
    drop(store);

    let view = TxnStore::recover_at_time(&wal, stamps[3]).expect("recover by time");
    assert_eq!(view.entries.len(), 4, "expected first four commits");
    for i in 0..4 {
        assert_eq!(
            view.entries
                .iter()
                .find(|(key, _)| key == format!("k{i}").as_bytes())
                .map(|(_, value)| value.as_slice()),
            Some(b"v1".as_slice())
        );
    }
}

#[test]
fn recover_at_time_without_index_is_rejected() {
    let dir = TempDir::new("txn-pitr-time-none");
    let wal = dir.join("time.wal");
    let store = TxnStore::open(&wal).expect("open");
    let mut txn = store.begin().expect("begin");
    txn.put(b"k", b"v").expect("put");
    txn.commit().expect("commit");
    drop(store);

    let index = tritdb_wal::time_index_path_for(&wal);
    std::fs::remove_file(&index).expect("remove index");

    let error = TxnStore::recover_at_time(&wal, u64::MAX).expect_err("index is gone");
    assert!(matches!(error, TxnError::TimeIndexUnavailable));
}

#[test]
fn time_index_survives_reset_and_reopen() {
    let dir = TempDir::new("txn-pitr-time-reset");
    let wal = dir.join("time.wal");
    let store = TxnStore::open(&wal).expect("open");
    let mut txn = store.begin().expect("begin");
    txn.put(b"before", b"1").expect("put");
    txn.commit().expect("commit");
    let before = now_ms();
    thread::sleep(Duration::from_millis(10));
    store.checkpoint().expect("checkpoint");
    drop(store);

    let store = TxnStore::open(&wal).expect("reopen");
    let mut txn = store.begin().expect("begin");
    txn.put(b"after", b"2").expect("commit");
    txn.commit().expect("commit");
    let after = now_ms();
    drop(store);

    let first = TxnStore::recover_at_time(&wal, before).expect("before checkpoint");
    assert_eq!(first.entries.len(), 1);
    let second = TxnStore::recover_at_time(&wal, after).expect("after checkpoint");
    assert_eq!(second.entries.len(), 2);
}
