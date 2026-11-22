use std::path::Path;

use tritdb_cas::{ChunkStore, Hash, Manifest, verify_chain};
use tritdb_txn::TxnStore;

use crate::Error;

use super::anchor;

pub fn restore_wal(db: &Path, cas_root: &Path, head: &str) -> Result<(), Error> {
    restore_wal_from(db, cas_root, Some(head), None).map(|_| ())
}

pub fn restore_wal_from(
    db: &Path,
    cas_root: &Path,
    head: Option<&str>,
    key: Option<&[u8]>,
) -> Result<String, Error> {
    let head = match head {
        Some(text) => Hash::from_hex(text).map_err(|error| self::error(cas_root, error))?,
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
    let chunks = ChunkStore::open(cas_root).map_err(|error| self::error(cas_root, error))?;
    verify_chain(&chunks, &head).map_err(|error| self::error(cas_root, error))?;
    let manifest = Manifest::load(&chunks, &head).map_err(|error| self::error(cas_root, error))?;
    let store = TxnStore::open(db).map_err(|error| self::error(db, error))?;
    let mut txn = store.begin().map_err(|error| self::error(db, error))?;
    for (key, _) in txn.iter().map_err(|error| self::error(db, error))? {
        let key_text = String::from_utf8_lossy(&key);
        if !manifest.entries.contains_key(key_text.as_ref()) {
            txn.delete(&key).map_err(|error| self::error(db, error))?;
        }
    }
    for (key, hash) in &manifest.entries {
        let value = chunks
            .get(hash)
            .map_err(|error| self::error(cas_root, error))?;
        txn.put(key.as_bytes(), &value)
            .map_err(|error| self::error(db, error))?;
    }
    txn.commit().map_err(|error| self::error(db, error))?;
    Ok(head.to_hex())
}

fn error(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
