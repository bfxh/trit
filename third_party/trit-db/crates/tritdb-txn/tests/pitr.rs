//! Point-in-time recovery over archived checkpoints.

use std::collections::BTreeMap;

use tritdb_testkit::TempDir;
use tritdb_txn::{TxnError, TxnStore};

fn put(store: &TxnStore, key: &str, value: &[u8]) -> u64 {
    let mut txn = store.begin().expect("begin");
    txn.put(key.as_bytes(), value).expect("put");
    txn.commit().expect("commit").commit_lsn
}

fn state_at(wal: &std::path::Path, lsn: u64) -> BTreeMap<Vec<u8>, Vec<u8>> {
    TxnStore::recover_at(wal, lsn)
        .expect("recover_at")
        .entries
        .into_iter()
        .collect()
}

#[test]
fn recover_at_returns_exact_state_for_each_archived_checkpoint() {
    let dir = TempDir::new("txn-pitr-checkpoints");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    let mut cp1_lsn = 0;
    for i in 0..10 {
        cp1_lsn = put(&store, &format!("k{i}"), b"v1");
    }
    let cp1 = store.checkpoint_archive().expect("archive checkpoint 1");
    assert_eq!(cp1.checkpoint_lsn, cp1_lsn);
    assert!(cp1.archived_wal_bytes.is_some());
    assert!(cp1.archived_snapshot_bytes.is_some());

    let mut cp2_lsn = 0;
    for i in 0..10 {
        cp2_lsn = put(&store, &format!("k{i}"), b"v2");
    }
    let cp2 = store.checkpoint_archive().expect("archive checkpoint 2");
    assert_eq!(cp2.checkpoint_lsn, cp2_lsn);

    for i in 0..5 {
        put(&store, &format!("k{i}"), b"v3");
    }
    drop(store);

    let at_cp1 = state_at(&wal, cp1.checkpoint_lsn);
    assert_eq!(at_cp1.len(), 10);
    for i in 0..10 {
        assert_eq!(at_cp1[format!("k{i}").as_bytes()], b"v1");
    }

    let at_cp2 = state_at(&wal, cp2.checkpoint_lsn);
    assert_eq!(at_cp2.len(), 10);
    for i in 0..10 {
        assert_eq!(at_cp2[format!("k{i}").as_bytes()], b"v2");
    }

    let at_end = state_at(&wal, u64::MAX);
    assert_eq!(at_end.len(), 10);
    for i in 0..5 {
        assert_eq!(at_end[format!("k{i}").as_bytes()], b"v3");
    }
    for i in 5..10 {
        assert_eq!(at_end[format!("k{i}").as_bytes()], b"v2");
    }
}

#[test]
fn recover_at_a_lsn_between_checkpoints_replays_only_commits_before_it() {
    let dir = TempDir::new("txn-pitr-between");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    for i in 0..4 {
        put(&store, &format!("k{i}"), b"v1");
    }
    let cp1 = store.checkpoint_archive().expect("archive checkpoint 1");

    // Three commits in the second batch. The second one is the target.
    let first = put(&store, "k0", b"v2");
    let second = put(&store, "k1", b"v2");
    let _third = put(&store, "k2", b"v2");

    let view = TxnStore::recover_at(&wal, second).expect("recover between");
    assert_eq!(view.checkpoint_lsn, cp1.checkpoint_lsn);
    assert_eq!(view.applied_lsn, second);
    assert!(second > first);

    let state: BTreeMap<_, _> = view.entries.into_iter().collect();
    assert_eq!(state[b"k0".as_slice()], b"v2");
    assert_eq!(state[b"k1".as_slice()], b"v2");
    assert_eq!(state[b"k2".as_slice()], b"v1");
    assert_eq!(state[b"k3".as_slice()], b"v1");
}

#[test]
fn recover_at_before_the_oldest_archive_is_rejected() {
    let dir = TempDir::new("txn-pitr-too-old");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    for i in 0..3 {
        put(&store, &format!("k{i}"), b"v1");
    }
    store.checkpoint_archive().expect("archive checkpoint");
    for i in 0..3 {
        put(&store, &format!("k{i}"), b"v2");
    }
    drop(store);

    let error = TxnStore::recover_at(&wal, 0).expect_err("target 0 predates the archive");
    match error {
        TxnError::PitrBeforeHistory { target_lsn, .. } => assert_eq!(target_lsn, 0),
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn plain_checkpoint_does_not_create_history() {
    let dir = TempDir::new("txn-pitr-plain");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    for i in 0..3 {
        put(&store, &format!("k{i}"), b"v1");
    }
    let info = store.checkpoint().expect("plain checkpoint");
    assert!(info.archived_wal_bytes.is_none());
    assert!(info.archived_snapshot_bytes.is_none());

    let history = {
        let mut name = wal.as_os_str().to_os_string();
        name.push(".history");
        std::path::PathBuf::from(name)
    };
    assert!(!history.exists());
}

#[test]
fn prune_history_keeps_only_the_newest_archives() {
    let dir = TempDir::new("txn-pitr-prune");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    let mut first_lsn = 0;
    for i in 0..2 {
        first_lsn = put(&store, &format!("a{i}"), b"1");
    }
    store.checkpoint_archive().expect("archive 1");

    for i in 0..2 {
        put(&store, &format!("b{i}"), b"2");
    }
    store.checkpoint_archive().expect("archive 2");

    let mut last_lsn = 0;
    for i in 0..2 {
        last_lsn = put(&store, &format!("c{i}"), b"3");
    }
    store.checkpoint_archive().expect("archive 3");
    drop(store);

    let info = TxnStore::prune_history(&wal, 2).expect("prune");
    assert_eq!(info.removed.len(), 1);
    assert_eq!(info.kept.len(), 2);

    let error = TxnStore::recover_at(&wal, first_lsn).expect_err("oldest archive removed");
    match error {
        TxnError::PitrBeforeHistory { target_lsn, .. } => assert_eq!(target_lsn, first_lsn),
        other => panic!("unexpected error: {other:?}"),
    }

    let view = TxnStore::recover_at(&wal, last_lsn).expect("newest archive survives");
    assert_eq!(view.entries.len(), 6);
}

#[test]
fn checkpoint_archive_keep_prunes_older_archives() {
    let dir = TempDir::new("txn-pitr-auto-gc");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    for round in 0..5 {
        let mut txn = store.begin().expect("begin");
        txn.put(format!("k{round}").as_bytes(), b"v").expect("put");
        txn.commit().expect("commit");
        let info = store.checkpoint_archive_keep(2).expect("archive");
        assert!(info.archived_wal_bytes.is_some());
    }
    drop(store);

    let mut history = wal.as_os_str().to_os_string();
    history.push(".history");
    let history = std::path::PathBuf::from(history);
    let wals = std::fs::read_dir(&history)
        .expect("history dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("wal"))
        .count();
    assert_eq!(wals, 2);
}

#[test]
fn checkpoint_archive_keep_zero_disables_pruning() {
    let dir = TempDir::new("txn-pitr-auto-gc-off");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open(&wal).expect("open");

    for round in 0..3 {
        let mut txn = store.begin().expect("begin");
        txn.put(format!("k{round}").as_bytes(), b"v").expect("put");
        txn.commit().expect("commit");
        let info = store.checkpoint_archive_keep(0).expect("archive");
        assert_eq!(info.pruned_archives, 0);
    }
    drop(store);

    let mut history = wal.as_os_str().to_os_string();
    history.push(".history");
    let history = std::path::PathBuf::from(history);
    let wals = std::fs::read_dir(&history)
        .expect("history dir")
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("wal"))
        .count();
    assert_eq!(wals, 3);
}

#[test]
fn multi_file_segments_are_archived_and_recoverable() {
    let dir = TempDir::new("txn-pitr-multifile");
    let wal = dir.join("pitr.wal");
    let store = TxnStore::open_with_wal_limit(&wal, 2048).expect("open");
    let payload = vec![7u8; 200];

    for i in 0..40 {
        let mut txn = store.begin().expect("begin");
        txn.put(format!("k{i}").as_bytes(), &payload).expect("put");
        txn.commit().expect("commit");
    }
    let segs = std::fs::read_dir(std::path::Path::new(&wal).parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".seg"))
        .count();
    assert!(segs > 0, "expected rotated segments before checkpoint");
    let info = store.checkpoint_archive().expect("archive multi-file wal");
    drop(store);

    let mut history = wal.as_os_str().to_os_string();
    history.push(".history");
    let archive = std::path::PathBuf::from(history).join(format!("{}.wal", info.checkpoint_lsn));
    let raw = std::fs::read(&archive).expect("read archive");
    let trws = raw
        .windows(4)
        .filter(|window| *window == tritdb_wal::SEGMENT_MAGIC)
        .count();
    assert!(trws > 1, "archive should contain several segment headers");
    let scan = tritdb_wal::WalReader::open(&archive).expect("scan archive");
    assert_eq!(scan.records.len(), 40 * 3, "begin+put+commit per txn");
    assert!(scan.segments > 1, "archive should contain several segments");

    let view = TxnStore::recover_at(&wal, u64::MAX).expect("recover");
    assert_eq!(view.entries.len(), 40);
}
