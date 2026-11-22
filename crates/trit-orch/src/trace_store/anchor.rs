//! 轨迹链头锚点：把最后一条事件的摘要用宿主密钥签名后落到 sidecar。
//!
//! 哈希链本身只能证明“文件内部没有被局部改过”；能改整个文件的攻击者可以把
//! 所有 payload 改掉再重算整条链。锚点是链的外部参照：`<path>.anchor` 里保存
//! 最后一条事件的 `chain_seq/seq/payload_hash`，并用宿主密钥做 HMAC-SHA256。
//! 只要密钥与锚点不在攻击者可写范围内，整链重算就会在 `load` 时暴露。

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::digest;
use crate::{Error, TraceEvent};

use super::TraceEventStore;

const ANCHOR_SCHEMA: u32 = 1;

#[derive(Clone, PartialEq, Eq)]
pub struct TraceAnchor {
    path: PathBuf,
    key: Vec<u8>,
    required: bool,
}

impl fmt::Debug for TraceAnchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TraceAnchor")
            .field("path", &self.path)
            .field("required", &self.required)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct AnchorRecord {
    schema: u32,
    file: String,
    chain_seq: u64,
    seq: u64,
    payload_hash: String,
    signature: String,
}

impl TraceAnchor {
    /// 可选锚点：锚点文件缺失时退化为无锚点（兼容旧日志）。
    pub fn new(path: impl Into<PathBuf>, key: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            key: key.into(),
            required: false,
        }
    }

    /// 严格锚点：非空日志没有锚点文件时直接拒绝。
    pub fn required(path: impl Into<PathBuf>, key: impl Into<Vec<u8>>) -> Self {
        Self {
            path: path.into(),
            key: key.into(),
            required: true,
        }
    }

    fn data_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".anchor");
        PathBuf::from(name)
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn signing_bytes(record: &AnchorRecord) -> Vec<u8> {
        let copy = AnchorRecord {
            schema: record.schema,
            file: record.file.clone(),
            chain_seq: record.chain_seq,
            seq: record.seq,
            payload_hash: record.payload_hash.clone(),
            signature: String::new(),
        };
        serde_json::to_vec(&copy).expect("anchor record is always serializable")
    }

    fn sign(&self, record: &mut AnchorRecord) {
        record.signature = digest::hex(&digest::hmac_sha256(
            &self.key,
            &Self::signing_bytes(record),
        ));
    }

    fn signature_valid(&self, record: &AnchorRecord) -> bool {
        !record.signature.is_empty()
            && record.signature
                == digest::hex(&digest::hmac_sha256(
                    &self.key,
                    &Self::signing_bytes(record),
                ))
    }

    pub(super) fn verify(&self, events: &[TraceEvent]) -> Result<(), Error> {
        let path = self.data_path();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if events.is_empty() || !self.required {
                    return Ok(());
                }
                return Err(Error::TraceAnchorMissing {
                    path: path.display().to_string(),
                });
            }
            Err(error) => {
                return Err(Error::TraceIo {
                    path: path.display().to_string(),
                    message: error.to_string(),
                });
            }
        };
        let record: AnchorRecord =
            serde_json::from_str(&text).map_err(|error| Error::TraceAnchorParse {
                path: path.display().to_string(),
                message: error.to_string(),
            })?;
        if !self.signature_valid(&record) {
            return Err(Error::TraceAnchorMismatch {
                path: path.display().to_string(),
            });
        }
        let Some(last) = events.iter().max_by_key(|event| event.chain_seq) else {
            return Err(Error::TraceAnchorMismatch {
                path: path.display().to_string(),
            });
        };
        if record.schema != ANCHOR_SCHEMA
            || record.file != self.file_name()
            || record.chain_seq != last.chain_seq
            || record.seq != last.seq
            || record.payload_hash != last.payload_hash
        {
            return Err(Error::TraceAnchorMismatch {
                path: path.display().to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn update(&self, event: &TraceEvent) -> Result<(), Error> {
        let mut record = AnchorRecord {
            schema: ANCHOR_SCHEMA,
            file: self.file_name(),
            chain_seq: event.chain_seq,
            seq: event.seq,
            payload_hash: event.payload_hash.clone(),
            signature: String::new(),
        };
        self.sign(&mut record);
        let text = serde_json::to_string(&record).map_err(|error| Error::TraceAnchorParse {
            path: self.data_path().display().to_string(),
            message: error.to_string(),
        })?;
        let path = self.data_path();
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut temporary = path.as_os_str().to_os_string();
        temporary.push(format!(".{}.{}.tmp", std::process::id(), seq));
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, text).map_err(|error| Error::TraceIo {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
        match std::fs::rename(&temporary, &path) {
            Ok(()) => Ok(()),
            Err(_) => {
                // Windows 不允许 rename 覆盖；先删再换。锚点坏了只会 fail-closed。
                let _ = std::fs::remove_file(&path);
                std::fs::rename(&temporary, &path).map_err(|error| Error::TraceIo {
                    path: path.display().to_string(),
                    message: error.to_string(),
                })
            }
        }
    }
}

impl TraceEventStore {
    pub(super) fn anchor_verify(&self, events: &[TraceEvent]) -> Result<(), Error> {
        match &self.anchor {
            Some(anchor) => anchor.verify(events),
            None => Ok(()),
        }
    }

    pub(super) fn anchor_update(&self, event: &TraceEvent) -> Result<(), Error> {
        match &self.anchor {
            Some(anchor) => anchor.update(event),
            None => Ok(()),
        }
    }
}
