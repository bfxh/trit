#![cfg(feature = "tritdb")]
//! 可选 trit-db 后端；key 布局见下方，HMAC 只防改数据库文件的攻击者。
#[cfg(feature = "tritdb-cas")]
pub mod cas;
#[cfg(feature = "tritdb-pgwire")]
pub mod pgwire;
#[cfg(feature = "tritdb-sql")]
pub mod pgwire_sqlite;
#[cfg(feature = "tritdb-sql")]
pub mod sql;
#[cfg(feature = "tritdb-sql")]
pub mod sqlite_materialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tritdb_txn::{TxnError, TxnStore};

use crate::digest;
use crate::{Error, TraceEvent, TraceLog, TraceStep};

const META_SCHEMA: u32 = 1;
const RETRIES: usize = 8;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Meta {
    schema: u32,
    chain_seq: u64,
    last_seq: u64,
    last_hash: String,
    watermark: u64,
    signature: String,
}

/// 签名输入：除 `signature` 外的全部字段。
fn unsigned(meta: &Meta) -> Vec<u8> {
    let mut copy = meta.clone();
    copy.signature.clear();
    serde_json::to_vec(&copy).unwrap()
}

enum AppendError {
    Conflict,
    Error(Error),
}

/// 使用 trit-db 的轨迹后端；API 与 [`crate::TraceEventStore`] 对齐。
pub struct TritDbTraceStore {
    path: PathBuf,
    store: TxnStore,
    key: Option<Vec<u8>>,
}

impl TritDbTraceStore {
    /// 打开/创建 trit-db WAL 文件。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_inner(path.as_ref(), None)
    }

    /// 打开并启用元数据 HMAC（可选外部锚点）。
    pub fn with_anchor(path: impl AsRef<Path>, key: impl AsRef<[u8]>) -> Result<Self, Error> {
        Self::open_inner(path.as_ref(), Some(key.as_ref().to_vec()))
    }

    fn open_inner(path: &Path, key: Option<Vec<u8>>) -> Result<Self, Error> {
        let store = TxnStore::open(path).map_err(|error| Error::TraceIo {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Ok(Self {
            path: path.to_path_buf(),
            store,
            key,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &TraceEvent) -> Result<(), Error> {
        self.append_batch(std::slice::from_ref(event))
    }

    pub fn append_step(
        &self,
        trace_id: &str,
        base_seq: u64,
        step: &TraceStep,
        observed_at: u64,
    ) -> Result<(), Error> {
        self.append_batch(&crate::events_from_step(
            trace_id,
            base_seq,
            step,
            observed_at,
        ))
    }

    pub fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        let entries = self.store.iter().map_err(|error| self.io(error))?;
        let mut events = Vec::new();
        let mut metas: BTreeMap<String, Meta> = BTreeMap::new();
        for (key, value) in entries {
            let Ok(key) = std::str::from_utf8(&key) else {
                continue;
            };
            if !key.starts_with("tr/") {
                continue;
            }
            if let Some(hex) = key
                .strip_prefix("tr/")
                .and_then(|rest| rest.strip_suffix("/m"))
            {
                let meta =
                    serde_json::from_slice::<Meta>(&value).map_err(|error| Error::TraceParse {
                        path: self.path.display().to_string(),
                        line: 0,
                        message: error.to_string(),
                    })?;
                metas.insert(hex.to_owned(), meta);
            } else {
                let event = serde_json::from_slice::<TraceEvent>(&value).map_err(|error| {
                    Error::TraceParse {
                        path: self.path.display().to_string(),
                        line: 0,
                        message: error.to_string(),
                    }
                })?;
                events.push(event);
            }
        }

        for event in &events {
            if event.payload_hash.is_empty() || event.payload_hash != event.computed_payload_hash()
            {
                return Err(Error::TraceHashMismatch {
                    path: self.path.display().to_string(),
                    line: 0,
                });
            }
        }

        let mut groups: BTreeMap<String, Vec<TraceEvent>> = BTreeMap::new();
        for event in events {
            groups
                .entry(event.trace_id.clone())
                .or_default()
                .push(event);
        }
        let mut output = Vec::new();
        for (trace_id, mut group) in groups {
            group.sort_by_key(|event| event.chain_seq);
            let mut previous = String::new();
            for (index, event) in group.iter().enumerate() {
                if event.chain_seq != (index + 1) as u64 || event.prev_hash != previous {
                    return Err(Error::TraceChainBroken {
                        path: self.path.display().to_string(),
                        seq: event.seq,
                    });
                }
                previous = event.payload_hash.clone();
            }
            let hex = digest::hex(trace_id.as_bytes());
            let Some(meta) = metas.get(&hex) else {
                return Err(Error::TraceParse {
                    path: self.path.display().to_string(),
                    line: 0,
                    message: format!("missing trit-db meta for trace {}", trace_id),
                });
            };
            if meta.schema != META_SCHEMA {
                return Err(Error::TraceParse {
                    path: self.path.display().to_string(),
                    line: 0,
                    message: format!("unsupported trit-db meta schema {}", meta.schema),
                });
            }
            let last = group.last().expect("group is non-empty");
            if meta.chain_seq != last.chain_seq
                || meta.last_seq != last.seq
                || meta.last_hash != last.payload_hash
            {
                return Err(Error::TraceChainBroken {
                    path: self.path.display().to_string(),
                    seq: last.seq,
                });
            }
            let max_seq = group
                .iter()
                .map(|event| event.seq)
                .max()
                .unwrap_or_default();
            if meta.watermark > max_seq {
                return Err(Error::TraceTruncated {
                    path: self.path.display().to_string(),
                    watermark: meta.watermark,
                    found: max_seq,
                });
            }
            self.verify_meta(meta)?;
            output.extend(group);
        }
        output.sort_by(|left, right| {
            left.trace_id
                .cmp(&right.trace_id)
                .then(left.seq.cmp(&right.seq))
        });
        Ok(output)
    }

    pub fn load_log(&self, trace_id: &str) -> Result<TraceLog, Error> {
        let mut log = TraceLog::new(trace_id);
        for event in self.load()? {
            if event.trace_id == trace_id {
                log.append(event)?;
            }
        }
        Ok(log)
    }

    fn append_batch(&self, events: &[TraceEvent]) -> Result<(), Error> {
        if events.is_empty() {
            return Ok(());
        }
        for _ in 0..RETRIES {
            match self.try_append(events) {
                Ok(()) => return Ok(()),
                Err(AppendError::Conflict) => continue,
                Err(AppendError::Error(error)) => return Err(error),
            }
        }
        Err(Error::TraceIo {
            path: self.path.display().to_string(),
            message: "trit-db write conflict after retries".into(),
        })
    }

    fn try_append(&self, events: &[TraceEvent]) -> Result<(), AppendError> {
        let mut txn = self
            .store
            .begin()
            .map_err(|error| AppendError::Error(self.io(error)))?;
        let mut metas: BTreeMap<String, Meta> = BTreeMap::new();
        for event in events {
            let existing = metas.get(&event.trace_id).cloned();
            let mut meta = match existing {
                Some(meta) => meta,
                None => {
                    let raw = txn
                        .get(&Self::meta_key(&event.trace_id))
                        .map_err(|error| AppendError::Error(self.io(error)))?;
                    match raw {
                        Some(bytes) => serde_json::from_slice::<Meta>(&bytes).map_err(|error| {
                            AppendError::Error(Error::TraceParse {
                                path: self.path.display().to_string(),
                                line: 0,
                                message: error.to_string(),
                            })
                        })?,
                        None => Meta::default(),
                    }
                }
            };
            meta.schema = META_SCHEMA;
            meta.chain_seq = meta.chain_seq.saturating_add(1);
            let mut sealed = event.clone();
            sealed.chain_seq = meta.chain_seq;
            sealed.seal(&meta.last_hash);
            meta.last_seq = sealed.seq;
            meta.last_hash = sealed.payload_hash.clone();
            meta.watermark = meta.watermark.max(sealed.seq);
            let key = Self::event_key(&event.trace_id, sealed.seq);
            let value = serde_json::to_vec(&sealed).map_err(|error| {
                AppendError::Error(Error::TraceParse {
                    path: self.path.display().to_string(),
                    line: 0,
                    message: error.to_string(),
                })
            })?;
            txn.put(&key, &value)
                .map_err(|error| AppendError::Error(self.io(error)))?;
            metas.insert(event.trace_id.clone(), meta);
        }
        for (trace_id, mut meta) in metas {
            self.sign_meta(&mut meta);
            let value = serde_json::to_vec(&meta).map_err(|error| {
                AppendError::Error(Error::TraceParse {
                    path: self.path.display().to_string(),
                    line: 0,
                    message: error.to_string(),
                })
            })?;
            txn.put(&Self::meta_key(&trace_id), &value)
                .map_err(|error| AppendError::Error(self.io(error)))?;
        }
        txn.commit().map_err(|error| match error {
            TxnError::Conflict { .. } => AppendError::Conflict,
            other => AppendError::Error(self.io(other)),
        })?;
        Ok(())
    }

    fn sign_meta(&self, meta: &mut Meta) {
        if let Some(key) = &self.key {
            meta.signature = digest::hex(&digest::hmac_sha256(key, &unsigned(meta)));
        }
    }

    fn verify_meta(&self, meta: &Meta) -> Result<(), Error> {
        if let Some(key) = &self.key {
            let expected = digest::hex(&digest::hmac_sha256(key, &unsigned(meta)));
            if meta.signature.is_empty() || meta.signature != expected {
                return Err(Error::TraceAnchorMismatch {
                    path: self.path.display().to_string(),
                });
            }
        }
        Ok(())
    }

    fn event_key(trace_id: &str, seq: u64) -> Vec<u8> {
        format!("tr/{}/e/{seq:020}", digest::hex(trace_id.as_bytes())).into_bytes()
    }

    fn meta_key(trace_id: &str) -> Vec<u8> {
        format!("tr/{}/m", digest::hex(trace_id.as_bytes())).into_bytes()
    }

    fn io(&self, error: TxnError) -> Error {
        Error::TraceIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        }
    }
}
