//! Transaction matrix: visibility, abort, conflicting commits, checkpoints,
//! recovery and delete persistence.

use std::fs;

use tritdb_testkit::TempDir;
use tritdb_txn::{TxnError, TxnStore};

fn open(name: &str) -> (TempDir, std::path::PathBuf, TxnStore) {
    let dir = TempDir::new(name);
    let wal = dir.join("txn.wal");
    let store = TxnStore::open(&wal).expect("open txn store");
    (dir, wal, store)
}

#[test]
fn put_get_overwrite_delete_matrix() {
    let (_dir, _wal, store) = open("txn-matrix-put");
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"1").expect("put a");
        txn.put(b"b", b"2").expect("put b");
        // Read-your-writes inside the transaction.
        assert_eq!(txn.get(b"a").expect("get a"), Some(b"1".to_vec()));
        txn.commit().expect("commit");
    }
    assert_eq!(store.get(b"a").expect("get a"), Some(b"1".to_vec()));
    assert_eq!(store.len().expect("len"), 2);
    assert_eq!(
        store.iter().expect("iter"),
        vec![
            (b"a".to_vec(), b"1".to_vec()),
            (b"b".to_vec(), b"2".to_vec())
        ]
    );

    // Overwriting a key does not grow the store.
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"9").expect("overwrite a");
        txn.commit().expect("commit");
    }
    assert_eq!(store.get(b"a").expect("get a"), Some(b"9".to_vec()));
    assert_eq!(store.len().expect("len"), 2);

    // Deleting a key removes it from reads and iteration.
    {
        let mut txn = store.begin().expect("begin");
        txn.delete(b"a").expect("delete a");
        txn.commit().expect("commit");
    }
    assert_eq!(store.get(b"a").expect("get a"), None);
    assert_eq!(store.len().expect("len"), 1);
    assert_eq!(
        store.iter().expect("iter"),
        vec![(b"b".to_vec(), b"2".to_vec())]
    );
    assert_eq!(store.active_txns(), 0);
}

#[test]
fn abort_discards_writes_and_store_stays_usable() {
    let (_dir, _wal, store) = open("txn-matrix-abort");
    let mut txn = store.begin().expect("begin");
    txn.put(b"k", b"v").expect("put");
    let aborted_lsn = txn.abort().expect("abort");
    assert!(aborted_lsn > 0);
    assert_eq!(store.get(b"k").expect("get"), None);
    assert_eq!(store.len().expect("len"), 0);
    assert_eq!(store.active_txns(), 0);

    // The store accepts new transactions after an abort.
    let mut txn = store.begin().expect("begin");
    txn.put(b"k", b"v2").expect("put");
    txn.commit().expect("commit");
    assert_eq!(store.get(b"k").expect("get"), Some(b"v2".to_vec()));
}

#[test]
fn uncommitted_writes_are_invisible_to_other_transactions() {
    let (_dir, _wal, store) = open("txn-matrix-isolation");
    let mut writer = store.begin().expect("begin writer");
    writer.put(b"k", b"uncommitted").expect("put");

    let reader = store.begin().expect("begin reader");
    assert_eq!(reader.get(b"k").expect("reader get"), None);
    drop(reader);

    writer.commit().expect("commit");
    assert_eq!(store.get(b"k").expect("get"), Some(b"uncommitted".to_vec()));
}

#[test]
fn conflicting_concurrent_commit_is_rejected() {
    let (_dir, _wal, store) = open("txn-matrix-conflict");
    let mut first = store.begin().expect("begin first");
    let mut second = store.begin().expect("begin second");
    first.put(b"k", b"first").expect("first put");
    second.put(b"k", b"second").expect("second put");

    first.commit().expect("first commit");
    let error = second.commit().expect_err("conflicting commit must fail");
    assert!(
        matches!(error, TxnError::Conflict { .. }),
        "unexpected error: {error:?}"
    );
    assert_eq!(store.get(b"k").expect("get"), Some(b"first".to_vec()));
    assert_eq!(store.len().expect("len"), 1);
}

#[test]
fn checkpoint_preserves_state_and_reopen_keeps_rows() {
    let (_dir, wal, store) = open("txn-matrix-checkpoint");
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"1").expect("put a");
        txn.put(b"b", b"2").expect("put b");
        txn.commit().expect("commit");
    }
    let wal_before = fs::metadata(&wal).expect("wal metadata").len();
    let info = store.checkpoint().expect("checkpoint");
    assert!(info.checkpoint_lsn > 0);
    let wal_after = fs::metadata(&wal).expect("wal metadata").len();
    assert!(
        wal_after < wal_before,
        "checkpoint should shrink the WAL ({wal_before} -> {wal_after})"
    );
    assert_eq!(store.get(b"a").expect("get a"), Some(b"1".to_vec()));
    assert_eq!(store.len().expect("len"), 2);

    drop(store);
    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(reopened.get(b"a").expect("get a"), Some(b"1".to_vec()));
    assert_eq!(reopened.get(b"b").expect("get b"), Some(b"2".to_vec()));
    assert_eq!(reopened.len().expect("len"), 2);
}

#[test]
fn recovery_applies_only_committed_transactions() {
    let (_dir, wal, store) = open("txn-matrix-recovery");
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"kept", b"1").expect("put kept");
        txn.commit().expect("commit");
    }
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"dropped", b"2").expect("put dropped");
        txn.abort().expect("abort");
    }
    drop(store);

    let recovered = TxnStore::open(&wal).expect("recover");
    assert_eq!(
        recovered.get(b"kept").expect("get kept"),
        Some(b"1".to_vec())
    );
    assert_eq!(recovered.get(b"dropped").expect("get dropped"), None);
    assert_eq!(recovered.len().expect("len"), 1);
}

#[test]
fn delete_is_persisted_across_reopen() {
    let (_dir, wal, store) = open("txn-matrix-delete");
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"1").expect("put a");
        txn.put(b"b", b"2").expect("put b");
        txn.commit().expect("commit");
    }
    {
        let mut txn = store.begin().expect("begin");
        txn.delete(b"a").expect("delete a");
        txn.commit().expect("commit");
    }
    drop(store);

    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(reopened.get(b"a").expect("get a"), None);
    assert_eq!(reopened.get(b"b").expect("get b"), Some(b"2".to_vec()));
    assert_eq!(reopened.len().expect("len"), 1);
}
