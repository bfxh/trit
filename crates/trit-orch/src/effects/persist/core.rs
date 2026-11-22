//! 台账的落盘、读回与重启恢复分类。只按落盘事实判断，不看内存里没保存的进度。

use serde::{Deserialize, Serialize};

use crate::{Effect, Error, Ledger};

use std::path::{Path, PathBuf};

#[path = "backup.rs"]
mod backup;

/// 台账的 JSON 落盘。写临时文件再改名，避免半截文件被当成有效台账。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerStore {
    path: PathBuf,
}

impl LedgerStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn io(&self, error: std::io::Error) -> Error {
        Error::LedgerIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        }
    }
}

impl Ledger {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("ledger is always serializable")
    }

    pub fn from_json(text: &str) -> Result<Self, Error> {
        serde_json::from_str(text).map_err(|error| Error::LedgerParse {
            message: error.to_string(),
        })
    }
}

/// 重启后的分类：只按**落盘事实**，不看内存里没保存的进度。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recovery {
    /// 从未发出，可安全发起。
    pub ready: Vec<String>,
    /// 可能已经发出，必须先对账。
    pub in_doubt: Vec<String>,
    /// 已确认未发生，可重试。
    pub retryable: Vec<String>,
    /// 已确认发生。
    pub done: Vec<String>,
}

impl Ledger {
    /// 重启后的恢复分类；顺序按 operation ID 升序，保证可复现。
    pub fn recover(&self) -> Recovery {
        let mut recovery = Recovery::default();
        for (operation, record) in &self.records {
            match record.state {
                Effect::Prepared => recovery.ready.push(operation.clone()),
                Effect::Sending | Effect::Sent | Effect::Unknown => {
                    recovery.in_doubt.push(operation.clone());
                }
                Effect::Refuted => recovery.retryable.push(operation.clone()),
                Effect::Confirmed => recovery.done.push(operation.clone()),
            }
        }
        recovery
    }
}
