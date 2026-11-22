//! 截断复核（#589）：映射建立后文件是否仍然至少和映射一样长。

use crate::{MappedFile, MmapError};

impl MappedFile {
    /// 复核底层文件当前长度，确认映射没有越过新的 EOF。
    ///
    /// 返回 `Ok(len)` 表示文件仍然至少和映射一样长；返回
    /// [`MmapError::Truncated`] 表示继续读 [`MappedFile::bytes`] 会 SIGBUS。
    ///
    /// 这只是**降低**风险，不是消除：`recheck_len` 返回后到实际读取之间仍可能被
    /// 外部截断（TOCTOU）。需要硬保证的消费方应改成堆内存副本。
    pub fn recheck_len(&self) -> Result<usize, MmapError> {
        let current = self.file.metadata()?.len();
        if current < self.len as u64 {
            return Err(MmapError::Truncated {
                mapped: self.len,
                current,
            });
        }
        Ok(self.len)
    }
}
