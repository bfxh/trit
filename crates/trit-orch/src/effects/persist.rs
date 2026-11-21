//! 台账的落盘、读回与重启恢复分类。只按落盘事实判断，不看内存里没保存的进度。

use serde::{Deserialize, Serialize};

use crate::{Effect, Error, Ledger};
// ---------------------------------------------------------------- 持久化与派发闸门

use std::path::{Path, PathBuf};

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

    /// 读台账；文件不存在视为空台账，其余错误照实返回。
    pub fn load(&self) -> Result<Ledger, Error> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => Ledger::from_json(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Ledger::new()),
            Err(error) => Err(self.io(error)),
        }
    }

    /// 原子替换式保存。
    pub fn save(&self, ledger: &Ledger) -> Result<(), Error> {
        let temporary = self.temporary_path();
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| self.io(error))?;
        }
        std::fs::write(&temporary, ledger.to_json()).map_err(|error| self.io(error))?;
        match std::fs::rename(&temporary, &self.path) {
            Ok(()) => Ok(()),
            // Windows 不允许 rename 覆盖已存在文件；先删再改名，窗口期由 tmp 文件兜底。
            Err(_) => {
                let _ = std::fs::remove_file(&self.path);
                std::fs::rename(&temporary, &self.path).map_err(|error| {
                    let _ = std::fs::remove_file(&temporary);
                    self.io(error)
                })
            }
        }
    }

    /// 每次保存都用进程内唯一的中转名。
    ///
    /// 只由目标路径派生（`path.with_extension("tmp")`）时，两个指向同一台账文件的
    /// `LedgerStore` 会写同一个 tmp 再各自 rename：一方可能把另一方的内容搬进自己的
    /// 路径，或第二次 rename 因源文件已消失而报错。调用方仍须自己串行化同一台账
    /// （§1.1 L3 口径不变），这里只保证「不会因同名中转而串内容」。
    fn temporary_path(&self) -> PathBuf {
        static SAVE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SAVE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let stem = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "ledger".into());
        self.path
            .with_file_name(format!("{stem}.{}.{}.tmp", std::process::id(), seq))
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
