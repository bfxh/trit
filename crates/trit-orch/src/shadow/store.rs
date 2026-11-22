//! 旧格式轨迹文件（一行一个 `TraceStep`）与整文件 HMAC 锚点。
//!
//! `TraceEventStore` 有逐事件哈希链；这里的一行一步格式没有链，所以用
//! “整文件 SHA-256 + 宿主 HMAC”的 sidecar 防止文件被替换/追加。它只提供
//! 完整性，不是完成闸证据。

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::digest;
use crate::{Error, TraceStep};

const SCHEMA: u32 = 1;

#[derive(Clone, PartialEq, Eq)]
struct ShadowAnchor {
    path: PathBuf,
    key: Vec<u8>,
    required: bool,
}

impl fmt::Debug for ShadowAnchor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ShadowAnchor")
            .field("path", &self.path)
            .field("required", &self.required)
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
struct ShadowRecord {
    schema: u32,
    file: String,
    sha256: String,
    signature: String,
}

impl ShadowAnchor {
    fn new(path: PathBuf, key: Vec<u8>, required: bool) -> Self {
        Self {
            path,
            key,
            required,
        }
    }

    fn data_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".shadow-anchor");
        PathBuf::from(name)
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn sign(&self, record: &mut ShadowRecord) {
        let copy = ShadowRecord {
            schema: record.schema,
            file: record.file.clone(),
            sha256: record.sha256.clone(),
            signature: String::new(),
        };
        record.signature = digest::hex(&digest::hmac_sha256(
            &self.key,
            &serde_json::to_vec(&copy).unwrap(),
        ));
    }

    fn signature_valid(&self, record: &ShadowRecord) -> bool {
        let copy = ShadowRecord {
            schema: record.schema,
            file: record.file.clone(),
            sha256: record.sha256.clone(),
            signature: String::new(),
        };
        !record.signature.is_empty()
            && record.signature
                == digest::hex(&digest::hmac_sha256(
                    &self.key,
                    &serde_json::to_vec(&copy).unwrap(),
                ))
    }

    fn verify(&self) -> Result<(), Error> {
        let data = self.data_path();
        let text = match std::fs::read_to_string(&data) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let log_missing = std::fs::metadata(&self.path)
                    .map(|metadata| metadata.len() == 0)
                    .unwrap_or(true);
                if log_missing || !self.required {
                    return Ok(());
                }
                return Err(Error::TraceAnchorMissing {
                    path: data.display().to_string(),
                });
            }
            Err(error) => {
                return Err(Error::TraceIo {
                    path: data.display().to_string(),
                    message: error.to_string(),
                });
            }
        };
        let record: ShadowRecord =
            serde_json::from_str(&text).map_err(|error| Error::TraceAnchorParse {
                path: data.display().to_string(),
                message: error.to_string(),
            })?;
        if !self.signature_valid(&record) {
            return Err(Error::TraceAnchorMismatch {
                path: data.display().to_string(),
            });
        }
        let bytes = std::fs::read(&self.path).map_err(|error| Error::TraceIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        })?;
        if record.schema != SCHEMA
            || record.file != self.file_name()
            || record.sha256 != digest::hex(&digest::sha256(&bytes))
        {
            return Err(Error::TraceAnchorMismatch {
                path: data.display().to_string(),
            });
        }
        Ok(())
    }

    fn update(&self) -> Result<(), Error> {
        let bytes = std::fs::read(&self.path).map_err(|error| Error::TraceIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        })?;
        let mut record = ShadowRecord {
            schema: SCHEMA,
            file: self.file_name(),
            sha256: digest::hex(&digest::sha256(&bytes)),
            signature: String::new(),
        };
        self.sign(&mut record);
        let text = serde_json::to_string(&record).map_err(|error| Error::TraceAnchorParse {
            path: self.data_path().display().to_string(),
            message: error.to_string(),
        })?;
        let data = self.data_path();
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let serial = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut temporary = data.as_os_str().to_os_string();
        temporary.push(format!(".{}.{}.tmp", std::process::id(), serial));
        let temporary = PathBuf::from(temporary);
        std::fs::write(&temporary, text).map_err(|error| Error::TraceIo {
            path: temporary.display().to_string(),
            message: error.to_string(),
        })?;
        match std::fs::rename(&temporary, &data) {
            Ok(()) => Ok(()),
            Err(_) => {
                let _ = std::fs::remove_file(&data);
                std::fs::rename(&temporary, &data).map_err(|error| Error::TraceIo {
                    path: data.display().to_string(),
                    message: error.to_string(),
                })
            }
        }
    }
}

/// 轨迹文件（JSONL，一行一个 `TraceStep`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceStore {
    path: PathBuf,
    anchor: Option<ShadowAnchor>,
}

impl TraceStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            anchor: None,
        }
    }

    pub fn with_anchor(path: impl Into<PathBuf>, key: impl AsRef<[u8]>) -> Self {
        let path = path.into();
        Self {
            path: path.clone(),
            anchor: Some(ShadowAnchor::new(path, key.as_ref().to_vec(), false)),
        }
    }

    pub fn with_anchor_required(path: impl Into<PathBuf>, key: impl AsRef<[u8]>) -> Self {
        let path = path.into();
        Self {
            path: path.clone(),
            anchor: Some(ShadowAnchor::new(path, key.as_ref().to_vec(), true)),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 追加一步。目录不存在会被创建；锚点更新失败时回滚这一步。
    pub fn append(&self, step: &TraceStep) -> Result<(), Error> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| self.io(error))?;
        }
        let previous_len = std::fs::metadata(&self.path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let line = serde_json::to_string(step).map_err(|error| Error::TraceParse {
            path: self.path.display().to_string(),
            line: 0,
            message: error.to_string(),
        })?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| self.io(error))?;
        writeln!(file, "{line}").map_err(|error| self.io(error))?;
        file.sync_data().map_err(|error| self.io(error))?;
        if let Some(anchor) = &self.anchor
            && let Err(error) = anchor.update()
        {
            let _ = file.set_len(previous_len);
            let _ = file.sync_all();
            return Err(error);
        }
        Ok(())
    }

    /// 读全部步骤；行号从 1 开始。配置锚点时先验整文件哈希与 HMAC。
    pub fn load(&self) -> Result<Vec<TraceStep>, Error> {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Some(anchor) = &self.anchor {
                    anchor.verify()?;
                }
                return Ok(Vec::new());
            }
            Err(error) => return Err(self.io(error)),
        };
        let mut steps = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let step = serde_json::from_str(line).map_err(|error| Error::TraceParse {
                path: self.path.display().to_string(),
                line: index + 1,
                message: error.to_string(),
            })?;
            steps.push(step);
        }
        if let Some(anchor) = &self.anchor {
            anchor.verify()?;
        }
        Ok(steps)
    }

    fn io(&self, error: std::io::Error) -> Error {
        Error::TraceIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        }
    }
}
