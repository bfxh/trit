//! CAS 快照：manifest 头 + anchor；具体实现拆到 snapshot/restore。

use std::path::Path;

#[path = "cas_anchor.rs"]
mod anchor;
#[path = "cas_restore.rs"]
mod restore;
#[path = "cas_snapshot.rs"]
mod snapshot;

pub use restore::{restore_wal, restore_wal_from};
pub use snapshot::{SnapshotInfo, snapshot_wal, snapshot_wal_with_key};

#[allow(dead_code)]
fn _path(_: &Path) {}
