//! 半行恢复：显式截掉最后一条不完整行（没有换行结尾的尾巴）。
//!
//! 这是破坏性操作，只由调用方在确认“最后一次写崩了”后执行；截断后水位可能
//! 仍高于实际最大 seq，`load` 会继续 fail-closed，需要调用方按业务决定是否重置。

use crate::Error;

use super::TraceEventStore;

impl TraceEventStore {
    /// 返回被丢弃的字节数；文件完整或不存在时返回 0。
    pub fn truncate_incomplete_tail(&self) -> Result<u64, Error> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(self.io(error)),
        };
        if bytes.is_empty() {
            return Ok(0);
        }
        let Some(position) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            let dropped = bytes.len() as u64;
            std::fs::write(&self.path, b"").map_err(|error| self.io(error))?;
            return Ok(dropped);
        };
        if position + 1 == bytes.len() {
            return Ok(0);
        }
        let dropped = (bytes.len() - position - 1) as u64;
        std::fs::write(&self.path, &bytes[..=position]).map_err(|error| self.io(error))?;
        Ok(dropped)
    }
}
