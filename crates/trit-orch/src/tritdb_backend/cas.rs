//! CAS 快照：manifest 头 + anchor；实现拆到 anchor/chain/snapshot/restore。

#[path = "cas_anchor.rs"]
mod anchor;
#[path = "cas_chain.rs"]
mod chain;
#[path = "cas_restore.rs"]
mod restore;
#[path = "cas_snapshot.rs"]
mod snapshot;

pub use restore::{restore_wal, restore_wal_from};
pub use snapshot::{SnapshotInfo, snapshot_wal, snapshot_wal_with_key};
