use std::path::Path;

use tritdb_cas::{ChunkStore, Hash, verify_chain};
use tritdb_txn::TxnStore;

use crate::Error;

use super::anchor;
use super::chain::{self, io};

pub fn restore_wal(db: &Path, cas_root: &Path, head: &str) -> Result<(), Error> {
    restore_wal_from(db, cas_root, Some(head), None).map(|_| ())
}

/// 沿 parent 链合并出完整快照，再把它写回 WAL；返回恢复到的头。
pub fn restore_wal_from(
    db: &Path,
    cas_root: &Path,
    head: Option<&str>,
    key: Option<&[u8]>,
) -> Result<String, Error> {
    let head = match head {
        Some(text) => Hash::from_hex(text).map_err(|error| io(cas_root, error))?,
        None => anchor::read_head(cas_root, key)?.ok_or_else(|| Error::TraceAnchorMissing {
            path: anchor::path(cas_root).display().to_string(),
        })?,
    };
    if key.is_some()
        && let Some(anchor_head) = anchor::read_head(cas_root, key)?
        && anchor_head != head
    {
        return Err(Error::TraceAnchorMismatch {
            path: anchor::path(cas_root).display().to_string(),
        });
    }
    let chunks = ChunkStore::open(cas_root).map_err(|error| io(cas_root, error))?;
    verify_chain(&chunks, &head).map_err(|error| io(cas_root, error))?;
    let (entries, _root) = chain::resolve(&chunks, &head)?;
    let store = TxnStore::open(db).map_err(|error| io(db, error))?;
    let mut txn = store.begin().map_err(|error| io(db, error))?;
    for (key, _) in txn.iter().map_err(|error| io(db, error))? {
        if !entries.contains_key(String::from_utf8_lossy(&key).as_ref()) {
            txn.delete(&key).map_err(|error| io(db, error))?;
        }
    }
    for (key, hash) in &entries {
        let value = chunks.get(hash).map_err(|error| io(cas_root, error))?;
        txn.put(key.as_bytes(), &value)
            .map_err(|error| io(db, error))?;
    }
    txn.commit().map_err(|error| io(db, error))?;
    Ok(head.to_hex())
}
