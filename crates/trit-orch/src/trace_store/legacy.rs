//! 旧格式兼容：没有内容摘要的历史事件按 `seq` 迁移成一条真实链；混合格式 fail-closed。

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use trit_ops::encode_line;

use crate::trace::{TraceEvent, TraceEventKind, TracePayload};
use crate::{Error, TraceStep};

use super::{MAX_TRACE_BYTES, TraceEventStore};

#[path = "legacy_migrate.rs"]
mod migrate;
pub(crate) use migrate::{has_content, migrate_in_memory};

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
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(self.io(error)),
        };
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for (index, line) in reader.lines().enumerate() {
            let line = line.map_err(|error| Error::TraceParse {
                path: self.path.display().to_string(),
                line: index + 1,
                message: error.to_string(),
            })?;
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
        let events = self.read_events()?;
        self.anchor_verify(&events)?;
        let events = migrate_in_memory(events);
        self.rewrite_events(&events)
    }

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
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        temporary.push(format!(".migrate.{}.{}.tmp", std::process::id(), seq));
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
