//! 旧格式兼容：没有内容摘要的历史事件按 `seq` 迁移成一条真实链；混合格式 fail-closed。

use std::fs;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use trit_ops::encode_line;

use crate::trace::{TraceEvent, TraceEventKind, TracePayload};
use crate::{Error, TraceStep};

use super::TraceEventStore;

#[path = "legacy_migrate.rs"]
mod migrate;
#[path = "read.rs"]
mod read;
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
    /// 把旧格式文件重写成按 seq 排序、带完整链的新格式。
    pub(super) fn migrate_legacy_file(&self) -> Result<(), Error> {
        let events = self.read_plain_events()?;
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
