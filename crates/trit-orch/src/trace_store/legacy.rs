//! 旧格式兼容：没有内容摘要的历史事件按 `seq` 迁移成一条真实链。
//!
//! 旧版本只写 payload，不带 `payload_hash` / `chain_seq`。升级后的读路径不能
//! 把一个合法旧文件判成“篡改”，否则公共内核的所有历史轨迹都会打不开。迁移
//! 只在**全文件都没有摘要**时发生；混合格式（部分有、部分无）仍 fail-closed。

use std::fs;
use std::io::Write;

use trit_ops::encode_line;

use crate::trace::{TraceEvent, TraceEventKind, TracePayload};
use crate::{Error, TraceStep};

use super::{MAX_TRACE_BYTES, TraceEventStore};

/// 旧 `TraceStep` 的迁移路径：拆成 plan / snapshot / limits / ledger 四条事件。
pub fn events_from_step(
    trace_id: &str,
    base_seq: u64,
    step: &TraceStep,
    observed_at: u64,
) -> Vec<TraceEvent> {
    let correlation = format!("legacy-step:{}", step.revision);
    vec![
        TraceEvent::new(
            trace_id,
            base_seq,
            TraceEventKind::PlanPatched,
            correlation.as_str(),
            observed_at,
        )
        .with_payload(TracePayload::Plan {
            revision: step.revision,
            tasks: step.tasks.clone(),
        }),
        TraceEvent::new(
            trace_id,
            base_seq.saturating_add(1),
            TraceEventKind::SnapshotCaptured,
            correlation.as_str(),
            observed_at,
        )
        .with_payload(TracePayload::Snapshot {
            snapshot: step.snapshot.clone(),
        }),
        TraceEvent::new(
            trace_id,
            base_seq.saturating_add(2),
            TraceEventKind::LimitsCaptured,
            correlation.as_str(),
            observed_at,
        )
        .with_payload(TracePayload::Limits {
            limits: step.limits.clone(),
        }),
        TraceEvent::new(
            trace_id,
            base_seq.saturating_add(3),
            TraceEventKind::LedgerCaptured,
            correlation.as_str(),
            observed_at,
        )
        .with_payload(TracePayload::Ledger {
            ledger: step.ledger.clone(),
        }),
    ]
}

impl TraceEventStore {
    /// 解析文件为原始事件（不校验摘要），供 load 与迁移共用。
    pub(super) fn read_events(&self) -> Result<Vec<TraceEvent>, Error> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(self.io(error)),
        };
        if metadata.len() > MAX_TRACE_BYTES {
            return Err(Error::TraceTooLarge {
                path: self.path.display().to_string(),
                size: metadata.len(),
                max: MAX_TRACE_BYTES,
            });
        }
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(self.io(error)),
        };
        let mut events = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let payload = self.decode_stored_line(index, line)?;
            let event: TraceEvent =
                serde_json::from_str(&payload).map_err(|error| Error::TraceParse {
                    path: self.path.display().to_string(),
                    line: index + 1,
                    message: error.to_string(),
                })?;
            if event.schema != TraceEvent::CURRENT_SCHEMA {
                return Err(Error::TraceSchema {
                    found: event.schema,
                    expected: TraceEvent::CURRENT_SCHEMA,
                });
            }
            events.push(event);
        }
        Ok(events)
    }

    /// 把旧格式文件重写成按 seq 排序、带完整链的新格式。
    pub(super) fn migrate_legacy_file(&self) -> Result<(), Error> {
        let events = migrate_in_memory(self.read_events()?);
        self.rewrite_events(&events)
    }

    /// 原子替换整份文件；保留 `.migrate.bak` 以便换入失败时回退。
    fn rewrite_events(&self, events: &[TraceEvent]) -> Result<(), Error> {
        let mut text = String::new();
        for event in events {
            let line = serde_json::to_string(event).map_err(|error| Error::TraceParse {
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
        }
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent).map_err(|error| self.io(error))?;
        }
        let mut temporary = self.path.as_os_str().to_os_string();
        temporary.push(".migrate.tmp");
        let temporary = std::path::PathBuf::from(temporary);
        let mut file = fs::File::create(&temporary).map_err(|error| self.io(error))?;
        file.write_all(text.as_bytes())
            .map_err(|error| self.io(error))?;
        file.sync_all().map_err(|error| self.io(error))?;

        let mut backup = self.path.as_os_str().to_os_string();
        backup.push(".migrate.bak");
        let backup = std::path::PathBuf::from(backup);
        if self.path.exists() {
            let _ = fs::remove_file(&backup);
            fs::rename(&self.path, &backup).map_err(|error| {
                let _ = fs::remove_file(&temporary);
                self.io(error)
            })?;
        }
        match fs::rename(&temporary, &self.path) {
            Ok(()) => {
                let _ = fs::remove_file(&backup);
                Ok(())
            }
            Err(error) => {
                let _ = fs::rename(&backup, &self.path);
                let _ = fs::remove_file(&temporary);
                Err(self.io(error))
            }
        }
    }
}

/// 内存迁移：按 seq 排序，写入序从 1 连续分配，逐条封存。
pub(super) fn migrate_in_memory(mut events: Vec<TraceEvent>) -> Vec<TraceEvent> {
    events.sort_by_key(|event| event.seq);
    let mut previous = String::new();
    for (index, event) in events.iter_mut().enumerate() {
        event.chain_seq = (index + 1) as u64;
        event.seal(&previous);
        previous = event.payload_hash.clone();
    }
    events
}

/// 文件里是否已有内容（存在但为空不算）。
pub(super) fn has_content(path: &std::path::Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false)
}
