use std::path::Path;

use serde::Serialize;
use tritdb_cas::{ChunkStore, Manifest};
use tritdb_txn::TxnStore;

use crate::Error;

use super::anchor;
use super::chain::{self, io};

#[derive(Debug, Clone, Serialize)]
pub struct SnapshotInfo {
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub anchor: String,
}

pub fn snapshot_wal(db: &Path, cas_root: &Path) -> Result<String, Error> {
    snapshot_wal_with_key(db, cas_root, None).map(|info| info.head)
}

/// 把 WAL 相对上一次 anchor 的变化写成新层，并把新头写进 anchor（可选 HMAC）。
///
/// 没有变化时不写新层，直接返回当前头。
pub fn snapshot_wal_with_key(
    db: &Path,
    cas_root: &Path,
    key: Option<&[u8]>,
) -> Result<SnapshotInfo, Error> {
    let parent = anchor::read_head(cas_root, key)?;
    let store = TxnStore::open(db).map_err(|error| io(db, error))?;
    let chunks = ChunkStore::open(cas_root).map_err(|error| io(cas_root, error))?;
    let current = chain::read_wal(&store, &chunks)?;
    let entries = match parent {
        Some(head) => chain::delta(&chunks, &head, &current)?,
        None => current,
    };
    let (head, parent_of_new) = if entries.is_empty() && parent.is_some() {
        (parent.expect("checked above"), None)
    } else {
        let head = Manifest::new(parent, entries)
            .store(&chunks)
            .map_err(|error| io(cas_root, error))?;
        (head, parent)
    };
    let anchor_path = anchor::write_head(cas_root, head, key)?;
    Ok(SnapshotInfo {
        head: head.to_hex(),
        parent: parent_of_new.map(|hash| hash.to_hex()),
        anchor: anchor_path.display().to_string(),
    })
}
