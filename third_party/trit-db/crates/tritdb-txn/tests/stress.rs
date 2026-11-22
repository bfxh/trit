//! Ignored stress tests. CI runs these in the nightly/manual stress job.

use std::sync::{Arc, Barrier};
use std::thread;

use tritdb_testkit::TempDir;
use tritdb_txn::TxnStore;

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

#[test]
#[ignore = "nightly stress: high thread count group commit"]
fn group_commit_many_threads() {
    let dir = TempDir::new("txn-stress-group");
    let wal = dir.join("stress.wal");
    let threads = env_usize("TRITDB_STRESS_THREADS", 32);
    let txns_per_thread = env_usize("TRITDB_STRESS_TXNS_PER_THREAD", 32);
    let total = threads * txns_per_thread;
    let barrier = Arc::new(Barrier::new(threads));
    let store = Arc::new(TxnStore::open(&wal).expect("open"));
    let mut handles = Vec::new();

    for thread_index in 0..threads {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for index in 0..txns_per_thread {
                let key = format!("s{thread_index}-{index}").into_bytes();
                let mut txn = store.begin().expect("begin");
                txn.put(&key, b"value").expect("put");
                txn.commit().expect("commit");
            }
        }));
    }
    for handle in handles {
        handle.join().expect("stress thread");
    }
    let syncs = store.sync_count();
    assert_eq!(store.len().expect("len"), total);
    assert!(syncs > 0 && syncs <= total as u64, "syncs={syncs}");
    drop(store);

    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(reopened.len().expect("len"), total);
    assert_eq!(reopened.recovery().committed_txns, total);
    assert_eq!(reopened.recovery().discarded_txns, 0);
    println!(
        "group stress: threads={threads} txns={total} syncs={syncs} ratio={:.3}",
        syncs as f64 / total as f64
    );
}

#[test]
#[ignore = "nightly stress: repeated checkpoint while commits are in flight"]
fn checkpoint_commit_loop() {
    let dir = TempDir::new("txn-stress-checkpoint");
    let wal = dir.join("stress.wal");
    let store = Arc::new(TxnStore::open(&wal).expect("open"));
    let rounds = env_usize("TRITDB_STRESS_ROUNDS", 100);
    let writer = {
        let store = Arc::clone(&store);
        thread::spawn(move || {
            for index in 0..rounds {
                let key = format!("k{index}").into_bytes();
                let mut txn = store.begin().expect("begin");
                txn.put(&key, b"value").expect("put");
                txn.commit().expect("commit");
            }
        })
    };
    let mut checkpoints = 0usize;
    let mut rejected = 0usize;
    while !writer.is_finished() {
        match store.checkpoint() {
            Ok(_) => checkpoints += 1,
            Err(tritdb_txn::TxnError::ActiveTxns { .. }) => rejected += 1,
            Err(error) => panic!("unexpected checkpoint error: {error}"),
        }
    }
    writer.join().expect("writer");
    store.checkpoint().expect("final checkpoint");
    drop(store);
    let reopened = TxnStore::open(&wal).expect("reopen");
    assert_eq!(reopened.len().expect("len"), rounds);
    println!("checkpoint loop: checkpoints={checkpoints} rejected={rejected}");
}
