//! 台账的原子替换与备份回退。目标损坏或缺失时都回退固定 `.bak`。

use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use crate::{Error, Ledger};

use super::LedgerStore;

impl LedgerStore {
    /// 读台账；目标不存在**或解析失败**时回退固定 `.bak`（save 在换文件窗口期
    /// 崩溃会留下它），两者都没有才视为空台账。
    pub fn load(&self) -> Result<Ledger, Error> {
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

    /// 原子替换式保存。
    ///
    /// 先写唯一 tmp 并 `sync_all`，再把当前有效台账挪到固定 `.bak`，最后把 tmp
    /// 换到目标。任何一步崩溃：目标缺失/损坏时 [`Self::load`] 回退 `.bak`，
    /// 最坏只丢最后一次未完成的写入。
    pub fn save(&self, ledger: &Ledger) -> Result<(), Error> {
        let temporary = self.temporary_path();
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(|error| self.io(error))?;
        }
        let mut file = std::fs::File::create(&temporary).map_err(|error| self.io(error))?;
        file.write_all(ledger.to_json().as_bytes())
            .map_err(|error| self.io(error))?;
        file.sync_all().map_err(|error| self.io(error))?;

        let backup = self.backup_path();
        if self.path.exists() {
            let _ = std::fs::remove_file(&backup);
            // 并发保存时目标可能已被另一线程挪走：没有旧文件可备份，继续换 tmp。
            if let Err(error) = std::fs::rename(&self.path, &backup)
                && error.kind() != ErrorKind::NotFound
            {
                let _ = std::fs::remove_file(&temporary);
                return Err(self.io(error));
            }
        }
        match std::fs::rename(&temporary, &self.path) {
            Ok(()) => {
                let _ = std::fs::remove_file(&backup);
                Ok(())
            }
            Err(error) => {
                // 换不上去就把旧台账原样放回，避免目标进入空窗。
                let _ = std::fs::rename(&backup, &self.path);
                let _ = std::fs::remove_file(&temporary);
                Err(self.io(error))
            }
        }
    }

    /// 每次保存都用进程内唯一的中转名。
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

    /// 固定备份名：与 tmp 不同，重启后仍能被 `load` 找到。
    fn backup_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".bak");
        PathBuf::from(name)
    }
}
