//! 批量追加：按路径分锁，读尾/链接/写盘/水位/锚点一次完成。
//!
//! 单条 append 与 `append_step` 的 4 条事件都走这里：一次锁、一次 `write_all`、
//! 一次 `sync_data`、一次水位与锚点更新。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use trit_ops::encode_line;

use crate::{Error, TraceEvent};

use super::{TraceEventStore, legacy};

static LOCKS: OnceLock<Mutex<BTreeMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();

fn lock_for(path: &Path) -> Arc<Mutex<()>> {
    let map = LOCKS.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut guard = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    guard
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

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

        let lock = lock_for(&self.path);
        let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

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
            self.write_watermark(last.seq)?;
            self.anchor_update(last)?;
        }
        Ok(())
    }
}
