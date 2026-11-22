//! CAS 链压缩：把 N 层 manifest 合并成单层，并记录被压缩前的 head。
//!
//! 压缩只改 anchor 指向：旧 manifest 仍在 CAS chunk 里，可凭
//! `<cas_root>/trit-trace.compacted` 里记录的 hash 手工恢复。

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tritdb_cas::{ChunkStore, Hash, Manifest, verify_chain};

use crate::Error;

use super::anchor;
use super::chain::{self, io};

#[derive(Debug, Clone, Serialize)]
pub struct CompactInfo {
    pub head: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
    pub layers: u64,
}

/// 把 `head`（或 anchor 当前头）所在的链压成单层；已是单层时不做任何事。
pub fn compact(
    cas_root: &Path,
    head: Option<&str>,
    key: Option<&[u8]>,
) -> Result<CompactInfo, Error> {
    let head = match head {
        Some(text) => Hash::from_hex(text).map_err(|error| io(cas_root, error))?,
        None => anchor::read_head(cas_root, key)?.ok_or_else(|| Error::TraceAnchorMissing {
            path: anchor::path(cas_root).display().to_string(),
        })?,
    };
    let chunks = ChunkStore::open(cas_root).map_err(|error| io(cas_root, error))?;
    let layers = verify_chain(&chunks, &head)
        .map_err(|error| io(cas_root, error))?
        .length;
    if layers <= 1 {
        return Ok(CompactInfo {
            head: head.to_hex(),
            previous: None,
            layers,
        });
    }

    let (entries, _root) = chain::resolve(&chunks, &head)?;
    let new_head = Manifest::new(None, entries)
        .store(&chunks)
        .map_err(|error| io(cas_root, error))?;
    record_compacted(cas_root, head)?;
    anchor::write_head(cas_root, new_head, key)?;
    Ok(CompactInfo {
        head: new_head.to_hex(),
        previous: Some(head.to_hex()),
        layers,
    })
}

/// 追加一行 `<unix 秒> <被压缩的 head>`，只增不改。
fn record_compacted(cas_root: &Path, previous: Hash) -> Result<(), Error> {
    let path = cas_root.join("trit-trace.compacted");
    let line = format!("{} {}\n", now_secs(), previous.to_hex());
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| io(&path, error))?;
    file.write_all(line.as_bytes())
        .map_err(|error| io(&path, error))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
