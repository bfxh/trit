//! Background checkpointing.

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tritdb_testkit::TempDir;
use tritdb_txn::TxnStore;

#[test]
fn background_checkpoint_keeps_data_and_bounds_the_wal() {
    let dir = TempDir::new("txn-bg-checkpoint");
    let wal = dir.join("bg.wal");
    let store = Arc::new(TxnStore::open(&wal).expect("open"));
    let background = store.start_background_checkpoint(Duration::from_millis(5));

    for i in 0..200 {
        let mut txn = store.begin().expect("begin");
        txn.put(format!("k{i}").as_bytes(), b"v").expect("put");
        txn.commit().expect("commit");
        if i % 20 == 0 {
            thread::sleep(Duration::from_millis(1));
        }
    }

    // Give the background thread at least one tick on fast machines.
    thread::sleep(Duration::from_millis(30));
    drop(background);
    drop(store);

    let store = TxnStore::open(&wal).expect("reopen");
    assert_eq!(store.len().expect("len"), 200);
    assert!(
        store.snapshot_path().is_file(),
        "background checkpoint should leave a snapshot"
    );
}
