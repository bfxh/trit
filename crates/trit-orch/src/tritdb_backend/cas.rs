//! 把 trit-db WAL 的 KV 快照进 CAS manifest，或从 manifest 恢复。
//!
//! 快照是内容寻址的：每个 value 写成 chunk，manifest 记录 key → chunk hash；
//! 可选地用 tritdb-cas 的 Anchor 对 manifest 头签名（CLI 暂未接线）。

use std::collections::BTreeMap;
use std::path::Path;

use tritdb_cas::{ChunkStore, Hash, Manifest};
use tritdb_txn::TxnStore;

use crate::Error;

/// 把 WAL 当前所有 KV 写进 CAS；返回 manifest 头 hash（hex）。
pub fn snapshot_wal(db: &Path, cas_root: &Path) -> Result<String, Error> {
    let store = TxnStore::open(db).map_err(|error| self::error(db, error))?;
    let chunks = ChunkStore::open(cas_root).map_err(|error| self::error(cas_root, error))?;
    let mut entries = BTreeMap::new();
    for (key, value) in store.iter().map_err(|error| self::error(db, error))? {
        let hash = chunks
            .put(&value)
            .map_err(|error| self::error(cas_root, error))?;
        entries.insert(String::from_utf8_lossy(&key).into_owned(), hash);
    }
    let manifest = Manifest::new(None, entries);
    let head = manifest
        .store(&chunks)
        .map_err(|error| self::error(cas_root, error))?;
    Ok(head.to_hex())
}

/// 从 CAS manifest 恢复 WAL；manifest 里没有的 key 会被删除。
pub fn restore_wal(db: &Path, cas_root: &Path, head: &str) -> Result<(), Error> {
    let head = Hash::from_hex(head).map_err(|error| self::error(cas_root, error))?;
    let chunks = ChunkStore::open(cas_root).map_err(|error| self::error(cas_root, error))?;
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
    Ok(())
}

fn error(path: &Path, error: impl std::fmt::Display) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
