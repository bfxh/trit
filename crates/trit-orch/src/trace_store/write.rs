//! 批量追加：按路径分锁，读尾/链接/写盘/水位/锚点一次完成。
//!
//! 单条 append 与 `append_step` 的 4 条事件都走这里：一次锁、一次 `write_all`、
//! 一次 `sync_data`、一次水位与锚点更新。

use std::fs;
use std::io::Write;

use trit_ops::encode_line;

use crate::{Error, TraceEvent};

use super::lock::{self, FileLock};
use super::{TraceEventStore, legacy};

impl TraceEventStore {
    pub(super) fn append_batch(&self, events: &[TraceEvent]) -> Result<(), Error> {
        if events.is_empty() {
            return Ok(());
        }
        for event in events {
            if event.schema != TraceEvent::CURRENT_SCHEMA {
                return Err(Error::TraceSchema {
                    found: event.schema,
                    expected: TraceEvent::CURRENT_SCHEMA,
                });
            }
        }

        let lock = lock::path_lock(&self.path);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let _file_lock = FileLock::acquire(&self.path)?;

        // 旧格式文件第一次被追加时先整体迁移，之后的链接才有前驱摘要。
        if legacy::has_content(&self.path) {
            let (last_chain, _) = self.last_tail()?;
            if last_chain == 0 {
                self.migrate_legacy_file()?;
            }
        }

        let (mut chain, mut previous) = self.last_tail()?;
        let mut text = String::new();
        let mut last_sealed = None;
        for event in events {
            chain = chain.saturating_add(1);
            let mut sealed = event.clone();
            self.stamp_event(&mut sealed);
            sealed.chain_seq = chain;
            sealed.seal(&previous);
            previous = sealed.payload_hash.clone();
            let line = serde_json::to_string(&sealed).map_err(|error| Error::TraceParse {
                path: self.path.display().to_string(),
                line: 0,
                message: error.to_string(),
            })?;
            let line = match self.ecc {
                Some(code) => encode_line(code, line.as_bytes()),
                None => line,
            };
            text.push_str(&line);
            text.push('\n');
            last_sealed = Some(sealed);
        }

        let previous_len = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let previous_watermark = self.read_watermark()?;
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| self.io(error))?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|error| self.io(error))?;
        file.write_all(text.as_bytes())
            .map_err(|error| self.io(error))?;
        file.sync_data().map_err(|error| self.io(error))?;

        if let Some(last) = &last_sealed {
            self.commit_sidecars(&mut file, previous_len, previous_watermark, last)?;
        }
        Ok(())
    }
}
