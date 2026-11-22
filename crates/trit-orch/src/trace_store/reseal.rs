//! 半行恢复后的显式重置：删除水位、按当前链尾重封锚点。
//!
//! 这两个动作是破坏性/显式的：调用方必须在确认“尾部已截断”后执行。它们也走
//! 跨进程锁，避免与 append 竞争。

use crate::Error;

use super::super::TraceEventStore;
use super::super::lock::{self, FileLock};

impl TraceEventStore {
    /// 删除水位文件；下一次 append 会重新建立水位。
    pub fn reset_watermark(&self) -> Result<(), Error> {
        let lock = lock::path_lock(&self.path);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_lock = FileLock::acquire(&self.path)?;
        let path = self.watermark_path();
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(Error::TraceIo {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }

    /// 按当前链尾重新封存锚点；截断掉旧尾巴后用它恢复锚点一致性。
    pub fn reseal_anchor(&self) -> Result<(), Error> {
        let lock = lock::path_lock(&self.path);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_lock = FileLock::acquire(&self.path)?;
        let events = self.read_plain_events()?;
        self.verify_chain(&events)?;
        if let Some(last) = events.iter().max_by_key(|event| event.chain_seq) {
            self.anchor_update(last)?;
        }
        Ok(())
    }
}
