//! Cross-process writer lock.

use tritdb_testkit::TempDir;
use tritdb_wal::{WalError, WalWriter};

#[test]
fn second_writer_is_rejected_until_the_first_drops() {
    let dir = TempDir::new("wal-lock");
    let path = dir.join("lock.wal");
    let (first, _) = WalWriter::open(&path).expect("first writer");

    let error = match WalWriter::open(&path) {
        Ok(_) => panic!("second writer must fail"),
        Err(error) => error,
    };
    assert!(matches!(error, WalError::Locked { .. }), "error: {error:?}");

    drop(first);
    let (_second, _) = WalWriter::open(&path).expect("lock released");
}
