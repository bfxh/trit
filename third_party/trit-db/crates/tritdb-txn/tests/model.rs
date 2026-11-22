//! Transaction-layer model tests, concurrency stress, and checkpoint races.

use std::collections::BTreeMap;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use tritdb_testkit::{Rng, TempDir};
use tritdb_txn::{TxnError, TxnStore};

fn entries(map: &BTreeMap<Vec<u8>, Vec<u8>>) -> Vec<(Vec<u8>, Vec<u8>)> {
    map.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn assert_matches(store: &TxnStore, model: &BTreeMap<Vec<u8>, Vec<u8>>, context: &str) {
    assert_eq!(
        store.iter().expect("read store"),
        entries(model),
        "store diverged at {context}"
    );
    assert_eq!(store.len().expect("len"), model.len(), "len at {context}");
}

#[test]
fn model_based_random_transactions_checkpoints_and_reopen() {
    let dir = TempDir::new("txn-model");
    let wal = dir.join("model.wal");
    let mut store = TxnStore::open(&wal).expect("open");
    let mut model = BTreeMap::new();
    let mut rng = Rng::new(0x7A11_0000);

    for step in 0..300 {
        let key = format!("k{}", rng.below(8)).into_bytes();
        let value = format!("v{step}-{}", rng.below(1000)).into_bytes();
        match rng.below(10) {
            0..=4 => {
                let mut txn = store.begin().expect("begin");
                txn.put(&key, &value).expect("put");
                txn.commit().expect("commit");
                model.insert(key, value);
            }
            5..=6 => {
                let mut txn = store.begin().expect("begin");
                txn.delete(&key).expect("delete");
                txn.commit().expect("commit");
                model.remove(&key);
            }
            7..=8 => {
                let key2 = format!("k{}", rng.below(8)).into_bytes();
                let mut txn = store.begin().expect("begin");
                txn.put(&key, &value).expect("put");
                txn.put(&key2, &value).expect("put");
                txn.commit().expect("commit");
                model.insert(key, value.clone());
                model.insert(key2, value);
            }
            _ => {
                let mut txn = store.begin().expect("begin");
                txn.put(&key, &value).expect("put");
                txn.abort().expect("abort");
                // Model unchanged.
            }
        }
        assert_eq!(store.active_txns(), 0, "active txn leak at step {step}");
        assert_matches(&store, &model, &format!("step {step}"));

        if step % 25 == 0 {
            store.checkpoint().expect("checkpoint");
            drop(store);
            store = TxnStore::open(&wal).expect("reopen");
            assert_matches(&store, &model, &format!("reopen {step}"));
        }
    }

    store.checkpoint().expect("final checkpoint");
    drop(store);
    let reopened = TxnStore::open(&wal).expect("final reopen");
    assert_matches(&reopened, &model, "final");
}

#[test]
fn checkpoint_cycles_recover_exact_state() {
    let dir = TempDir::new("txn-cycles");
    let wal = dir.join("cycles.wal");
    let mut store = TxnStore::open(&wal).expect("open");
    let mut model = BTreeMap::new();

    for cycle in 0..20u64 {
        for index in 0..10u64 {
            let key = format!("c{cycle}-{index}").into_bytes();
            let value = format!("value-{cycle}-{index}").into_bytes();
            let mut txn = store.begin().expect("begin");
            txn.put(&key, &value).expect("put");
            txn.commit().expect("commit");
            model.insert(key, value);
        }
        store.checkpoint().expect("checkpoint");
        drop(store);
        store = TxnStore::open(&wal).expect("reopen");
        assert_matches(&store, &model, &format!("cycle {cycle}"));
    }
}

#[test]
fn checkpoint_after_reopen_without_writes_preserves_lsn() {
    let dir = TempDir::new("txn-lsn");
    let wal = dir.join("lsn.wal");
    let store = TxnStore::open(&wal).expect("open");
    let mut txn = store.begin().expect("begin");
    txn.put(b"alpha", b"one").expect("put");
    txn.commit().expect("commit");
    let first = store.checkpoint().expect("first checkpoint");
    assert!(first.checkpoint_lsn > 0);
    drop(store);

    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(reopened.recovery().checkpoint_lsn, first.checkpoint_lsn);
    let second = reopened.checkpoint().expect("checkpoint without writes");
    assert_eq!(
        second.checkpoint_lsn, first.checkpoint_lsn,
        "checkpoint must not move its LSN backwards after a snapshot-only open"
    );
    assert_eq!(second.entries, 1);
    drop(reopened);

    let final_store = TxnStore::open(&wal).expect("final reopen");
    assert_eq!(
        final_store.get(b"alpha").expect("get"),
        Some(b"one".to_vec())
    );
}

#[test]
fn concurrent_group_commit_stress_recovers_all_rows() {
    let dir = TempDir::new("txn-group-stress");
    let wal = dir.join("group.wal");
    let threads = 16usize;
    let txns_per_thread = 16usize;
    let barrier = Arc::new(Barrier::new(threads));
    let store = Arc::new(TxnStore::open(&wal).expect("open"));
    let mut handles = Vec::new();

    for thread_index in 0..threads {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || -> Result<(), String> {
            barrier.wait();
            for index in 0..txns_per_thread {
                let key = format!("t{thread_index}-{index}").into_bytes();
                let value = format!("v{thread_index}-{index}").into_bytes();
                let mut txn = store.begin().map_err(|error| error.to_string())?;
                txn.put(&key, &value).map_err(|error| error.to_string())?;
                txn.commit().map_err(|error| error.to_string())?;
            }
            Ok(())
        }));
    }
    for handle in handles {
        handle.join().expect("thread").expect("transaction");
    }

    assert_eq!(store.active_txns(), 0);
    let syncs = store.sync_count();
    assert!(syncs > 0 && syncs < (threads * txns_per_thread) as u64);
    assert_eq!(store.len().expect("len"), threads * txns_per_thread);
    let entries_before = store.iter().expect("entries");
    drop(store);

    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(
        reopened.recovery().committed_txns,
        threads * txns_per_thread
    );
    assert_eq!(reopened.recovery().discarded_txns, 0);
    assert_eq!(reopened.iter().expect("entries"), entries_before);
}

#[test]
fn readers_observe_monotonic_committed_values() {
    let dir = TempDir::new("txn-readers");
    let wal = dir.join("readers.wal");
    let store = Arc::new(TxnStore::open(&wal).expect("open"));
    {
        let mut txn = store.begin().expect("begin");
        txn.put(b"counter", b"0").expect("put");
        txn.commit().expect("commit");
    }

    let reader_count = 4usize;
    let reads_per_reader = 500usize;
    let mut readers = Vec::new();
    for _ in 0..reader_count {
        let store = Arc::clone(&store);
        readers.push(thread::spawn(move || {
            let mut last = 0u64;
            for _ in 0..reads_per_reader {
                let value = store.get(b"counter").expect("get").expect("counter exists");
                let current: u64 = std::str::from_utf8(&value)
                    .expect("utf8")
                    .parse()
                    .expect("number");
                assert!(
                    current >= last,
                    "reader went backwards: {last} -> {current}"
                );
                last = current;
            }
        }));
    }

    let writer_store = Arc::clone(&store);
    let writer = thread::spawn(move || {
        for index in 1..=200u64 {
            let mut txn = writer_store.begin().expect("begin");
            txn.put(b"counter", index.to_string().as_bytes())
                .expect("put");
            txn.commit().expect("commit");
        }
    });
    writer.join().expect("writer");
    for reader in readers {
        reader.join().expect("reader");
    }
    assert_eq!(store.get(b"counter").expect("get"), Some(b"200".to_vec()));
}

#[test]
fn checkpoint_race_with_commits_is_safe() {
    let dir = TempDir::new("txn-checkpoint-race");
    let wal = dir.join("race.wal");
    let store = Arc::new(TxnStore::open(&wal).expect("open"));
    let done = Arc::new(AtomicBool::new(false));

    let writer_store = Arc::clone(&store);
    let writer = thread::spawn(move || {
        for index in 0..200u64 {
            let key = format!("r{index}").into_bytes();
            let mut txn = writer_store.begin().expect("begin");
            txn.put(&key, b"value").expect("put");
            txn.commit().expect("commit");
            thread::yield_now();
        }
    });

    let checkpoint_store = Arc::clone(&store);
    let checkpoint_done = Arc::clone(&done);
    let checkpointer = thread::spawn(move || {
        let mut successes = 0usize;
        for _ in 0..50 {
            if checkpoint_done.load(Ordering::Relaxed) {
                break;
            }
            match checkpoint_store.checkpoint() {
                Ok(_) => successes += 1,
                Err(TxnError::ActiveTxns { .. }) => {}
                Err(error) => panic!("unexpected checkpoint error: {error}"),
            }
            thread::yield_now();
        }
        successes
    });

    writer.join().expect("writer");
    done.store(true, Ordering::Relaxed);
    let successes = checkpointer.join().expect("checkpointer");
    println!("concurrent checkpoints that succeeded: {successes}");
    let final_checkpoint = store.checkpoint().expect("final checkpoint");
    assert_eq!(final_checkpoint.entries, 200);
    drop(store);
    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(reopened.len().expect("len"), 200);
    assert_eq!(
        reopened.recovery().committed_txns,
        0,
        "final checkpoint moved applied state into the snapshot"
    );
    assert_eq!(reopened.recovery().records, 0);
    assert!(reopened.recovery().checkpoint_lsn > 0);
}

#[test]
fn snapshot_truncation_is_rejected_at_every_cut() {
    let dir = TempDir::new("txn-snapshot-trunc");
    let wal = dir.join("snap.wal");
    let store = TxnStore::open(&wal).expect("open");
    for index in 0..5u64 {
        let mut txn = store.begin().expect("begin");
        txn.put(format!("k{index}").as_bytes(), b"value")
            .expect("put");
        txn.commit().expect("commit");
    }
    store.checkpoint().expect("checkpoint");
    let snapshot = store.snapshot_path().to_path_buf();
    let raw = fs::read(&snapshot).expect("read snapshot");
    drop(store);

    for cut in 0..raw.len() {
        fs::write(&snapshot, &raw[..cut]).expect("write truncated snapshot");
        let error = TxnStore::open(&wal).err().expect("truncated snapshot");
        assert!(
            matches!(error, TxnError::BadSnapshot { .. }),
            "cut {cut}: {error:?}"
        );
    }
    fs::write(&snapshot, &raw).expect("restore snapshot");
    TxnStore::open(&wal).expect("valid snapshot must still open");
}

#[test]
fn snapshot_crc_corruption_is_rejected() {
    let dir = TempDir::new("txn-snapshot-crc");
    let wal = dir.join("snap.wal");
    let store = TxnStore::open(&wal).expect("open");
    let mut txn = store.begin().expect("begin");
    txn.put(b"key", b"value").expect("put");
    txn.commit().expect("commit");
    store.checkpoint().expect("checkpoint");
    let snapshot = store.snapshot_path().to_path_buf();
    let raw = fs::read(&snapshot).expect("read snapshot");
    drop(store);

    for index in 0..raw.len() {
        let mut mutated = raw.clone();
        mutated[index] ^= 0xff;
        fs::write(&snapshot, &mutated).expect("write mutated snapshot");
        let error = TxnStore::open(&wal).err().expect("corrupted snapshot");
        assert!(
            matches!(error, TxnError::BadSnapshot { .. }),
            "byte {index}: {error:?}"
        );
    }
    fs::write(&snapshot, &raw).expect("restore snapshot");
    TxnStore::open(&wal).expect("valid snapshot must still open");
}

#[test]
fn empty_keys_and_values_are_supported() {
    let dir = TempDir::new("txn-empty");
    let wal = dir.join("empty.wal");
    let store = TxnStore::open(&wal).expect("open");
    let mut txn = store.begin().expect("begin");
    txn.put(b"", b"").expect("put empty");
    txn.commit().expect("commit");
    assert_eq!(store.get(b"").expect("get"), Some(Vec::new()));
    let mut txn = store.begin().expect("begin");
    txn.delete(b"missing").expect("delete missing");
    txn.commit().expect("commit");
    assert_eq!(store.len().expect("len"), 1);
}

#[test]
fn txn_ids_are_unique_and_increasing() {
    let dir = TempDir::new("txn-ids");
    let wal = dir.join("ids.wal");
    let store = TxnStore::open(&wal).expect("open");
    let mut previous = 0u64;
    for index in 0..100u64 {
        let mut txn = store.begin().expect("begin");
        let id = txn.id();
        txn.put(format!("k{index}").as_bytes(), b"v").expect("put");
        txn.commit().expect("commit");
        assert_eq!(id, previous + 1, "txn id sequence");
        previous = id;
    }
    assert_eq!(store.active_txns(), 0);
}

#[test]
fn checkpoint_on_empty_store_is_valid() {
    let dir = TempDir::new("txn-empty-checkpoint");
    let wal = dir.join("empty.wal");
    let store = TxnStore::open(&wal).expect("open");
    let info = store.checkpoint().expect("checkpoint");
    assert_eq!(info.entries, 0);
    assert_eq!(info.wal_bytes_after, 32);
    drop(store);
    let reopened = TxnStore::open(&wal).expect("reopen");
    assert!(reopened.is_empty().expect("empty"));
    assert_eq!(reopened.recovery().committed_txns, 0);
    assert_eq!(reopened.recovery().records, 0);
}
