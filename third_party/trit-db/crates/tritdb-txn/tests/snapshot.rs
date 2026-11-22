//! Snapshot isolation: stable reads, read-your-writes, first-committer-wins.

use tritdb_testkit::TempDir;
use tritdb_txn::{TxnError, TxnStore};

#[test]
fn txn_reads_a_stable_snapshot_and_reads_its_own_writes() {
    let dir = TempDir::new("txn-snapshot");
    let wal = dir.join("s.wal");
    let store = TxnStore::open(&wal).expect("open");

    let mut seed = store.begin().expect("begin");
    seed.put(b"k", b"v1").expect("put");
    seed.commit().expect("commit");

    let mut reader = store.begin().expect("begin");
    assert_eq!(reader.get(b"k").expect("get"), Some(b"v1".to_vec()));

    let mut other = store.begin().expect("begin");
    other.put(b"k", b"v2").expect("put");
    other.commit().expect("commit");

    // The reader's snapshot is stable.
    assert_eq!(reader.get(b"k").expect("get"), Some(b"v1".to_vec()));
    assert_eq!(store.get(b"k").expect("get"), Some(b"v2".to_vec()));

    // Read-your-writes.
    reader.put(b"k", b"v3").expect("put");
    assert_eq!(reader.get(b"k").expect("get"), Some(b"v3".to_vec()));

    // First-committer-wins.
    let error = reader.commit().expect_err("write conflict");
    assert!(
        matches!(error, TxnError::Conflict { .. }),
        "error: {error:?}"
    );
    assert_eq!(store.get(b"k").expect("get"), Some(b"v2".to_vec()));
}

#[test]
fn concurrent_writes_to_different_keys_both_commit() {
    let dir = TempDir::new("txn-snapshot-disjoint");
    let store = TxnStore::open(&dir.join("s.wal")).expect("open");

    let mut left = store.begin().expect("begin");
    let mut right = store.begin().expect("begin");
    left.put(b"a", b"1").expect("put");
    right.put(b"b", b"2").expect("put");
    left.commit().expect("commit left");
    right.commit().expect("commit right");

    assert_eq!(store.get(b"a").expect("get"), Some(b"1".to_vec()));
    assert_eq!(store.get(b"b").expect("get"), Some(b"2".to_vec()));
}

#[test]
fn txn_iter_merges_snapshot_and_buffered_ops() {
    let dir = TempDir::new("txn-snapshot-iter");
    let store = TxnStore::open(&dir.join("s.wal")).expect("open");

    let mut seed = store.begin().expect("begin");
    seed.put(b"a", b"1").expect("put");
    seed.commit().expect("commit");

    let mut txn = store.begin().expect("begin");
    txn.put(b"b", b"2").expect("put");
    txn.delete(b"a").expect("delete");

    let entries = txn.iter().expect("iter");
    assert_eq!(entries, vec![(b"b".to_vec(), b"2".to_vec())]);
}
