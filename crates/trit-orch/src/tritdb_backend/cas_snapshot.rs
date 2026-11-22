use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use tritdb_cas::{ChunkStore, Manifest};
use tritdb_txn::TxnStore;

use crate::Error;

use super::anchor;

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

pub fn snapshot_wal_with_key(
    db: &Path,
    cas_root: &Path,
    key: Option<&[u8]>,
) -> Result<SnapshotInfo, Error> {
    let parent = anchor::read_head(cas_root, key)?;
    let store = TxnStore::open(db).map_err(|error| self::error(db, error))?;
    let chunks = ChunkStore::open(cas_root).map_err(|error| self::error(cas_root, error))?;
    let mut entries = BTreeMap::new();
    for (key, value) in store.iter().map_err(|error| self::error(db, error))? {
        let hash = chunks
            .put(&value)
            .map_err(|error| self::error(cas_root, error))?;
        entries.insert(String::from_utf8_lossy(&key).into_owned(), hash);
    }
    let manifest = Manifest::new(parent, entries);
    let head = manifest
        .store(&chunks)
        .map_err(|error| self::error(cas_root, error))?;
    let anchor_path = anchor::write_head(cas_root, head, key)?;
    Ok(SnapshotInfo {
        head: head.to_hex(),
        parent: parent.map(|hash| hash.to_hex()),
        anchor: anchor_path.display().to_string(),
    })
}

fn error(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
