//! 增量层：内容没变就不写新层，恢复仍得到同一份快照。

use trit_orch::tritdb_backend::{TritDbTraceStore, cas};

use super::{event, temp_path};

#[test]
fn unchanged_snapshot_does_not_write_a_layer() {
    let db = temp_path("same", ".wal");
    let cas_root = temp_path("same-cas", "");
    let _ = std::fs::remove_dir_all(&cas_root);
    let key = b"same-key";

    let store = TritDbTraceStore::open(&db).unwrap();
    store.append(&event(1)).unwrap();
    drop(store);
    let first = cas::snapshot_wal_with_key(&db, &cas_root, Some(key)).unwrap();

    let again = cas::snapshot_wal_with_key(&db, &cas_root, Some(key)).unwrap();
    assert_eq!(again.head, first.head);
    assert_eq!(again.parent, first.parent);

    let restored = temp_path("same-restored", ".wal");
    let head = cas::restore_wal_from(&restored, &cas_root, None, Some(key)).unwrap();
    assert_eq!(head, first.head);
    let loaded = TritDbTraceStore::open(&restored).unwrap().load().unwrap();
    assert_eq!(loaded.len(), 1);

    let _ = std::fs::remove_file(&restored);
    let _ = std::fs::remove_file(&db);
    let _ = std::fs::remove_dir_all(&cas_root);
}
