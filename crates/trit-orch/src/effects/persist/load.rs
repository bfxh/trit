//! 读台账：目标损坏或缺失时回退固定 `.bak`；读也持锁，避免读到换文件的窗口。

use std::io::ErrorKind;

use crate::{Error, Ledger};

use super::super::LedgerStore;

impl LedgerStore {
    /// 读台账；目标不存在**或解析失败**时回退固定 `.bak`（save 在换文件窗口期
    /// 崩溃会留下它），两者都没有才视为空台账。
    pub fn load(&self) -> Result<Ledger, Error> {
        let _file_lock = self.lock()?;
        match std::fs::read_to_string(&self.path) {
            Ok(text) => {
                Ledger::from_json(&text).or_else(|primary| self.load_backup().or(Err(primary)))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                self.load_backup().or_else(|_| Ok(Ledger::new()))
            }
            Err(error) => Err(self.io(error)),
        }
    }

    fn load_backup(&self) -> Result<Ledger, Error> {
        let backup = self.backup_path();
        match std::fs::read_to_string(&backup) {
            Ok(text) => Ledger::from_json(&text),
            Err(error) => Err(Error::LedgerIo {
                path: backup.display().to_string(),
                message: error.to_string(),
            }),
        }
    }
}
