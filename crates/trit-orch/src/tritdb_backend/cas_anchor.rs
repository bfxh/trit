//! CAS 快照的 anchor sidecar：记录 manifest 头，并可选做 HMAC。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tritdb_cas::{Anchor, Hash};

use crate::Error;

pub(super) fn path(cas_root: &Path) -> PathBuf {
    cas_root.join("trit-trace.anchor")
}

pub(super) fn read_head(cas_root: &Path, key: Option<&[u8]>) -> Result<Option<Hash>, Error> {
    let path = self::path(cas_root);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io(&path, error)),
    };
    let anchor = Anchor::decode(&bytes).map_err(|error| Error::TraceAnchorParse {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    match (key, anchor.hmac.is_some()) {
        (Some(key), true) if anchor.verify(key) => {}
        (None, false) => {}
        _ => {
            return Err(Error::TraceAnchorMismatch {
                path: path.display().to_string(),
            });
        }
    }
    Ok(Some(anchor.head))
}

pub(super) fn write_head(
    cas_root: &Path,
    head: Hash,
    key: Option<&[u8]>,
) -> Result<PathBuf, Error> {
    let path = self::path(cas_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| io(&path, error))?;
    }
    let mut anchor = Anchor::new(head);
    if let Some(key) = key {
        anchor.sign(key);
    }
    let mut temporary = path.as_os_str().to_os_string();
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let serial = SEQ.fetch_add(1, Ordering::Relaxed);
    temporary.push(format!(".{}.{}.tmp", std::process::id(), serial));
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, anchor.encode()).map_err(|error| io(&temporary, error))?;
    match std::fs::rename(&temporary, &path) {
        Ok(()) => Ok(path),
        Err(_) => {
            let _ = std::fs::remove_file(&path);
            std::fs::rename(&temporary, &path).map_err(|error| io(&path, error))?;
            Ok(path)
        }
    }
}

fn io(path: &Path, error: std::io::Error) -> Error {
    Error::TraceIo {
        path: path.display().to_string(),
        message: error.to_string(),
    }
}
