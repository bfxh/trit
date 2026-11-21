//! 轨迹事件的 JSONL 持久化，以及旧 `TraceStep` 到事件流的迁移。

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use crate::trace::{TraceEvent, TraceEventKind, TraceLog, TracePayload};
use crate::{Error, TraceStep};

/// 一行一个 `TraceEvent` 的追加式存储。
///
/// 同一文件的并发追加由宿主串行化；这里只保证单次 append 的字节完整落盘。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEventStore {
    path: PathBuf,
}

impl TraceEventStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&self, event: &TraceEvent) -> Result<(), Error> {
        if event.schema != TraceEvent::CURRENT_SCHEMA {
            return Err(Error::TraceSchema {
                found: event.schema,
                expected: TraceEvent::CURRENT_SCHEMA,
            });
        }
        let line = serde_json::to_string(event).map_err(|error| Error::TraceParse {
            path: self.path.display().to_string(),
            line: 0,
            message: error.to_string(),
        })?;
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
        writeln!(file, "{line}").map_err(|error| self.io(error))?;
        // 审计事件按“写成功即可见”处理：每次追加都同步到存储。
        file.sync_data().map_err(|error| self.io(error))
    }

    pub fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(self.io(error)),
        };
        let mut events = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let event: TraceEvent =
                serde_json::from_str(line).map_err(|error| Error::TraceParse {
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

    /// 读出指定 trace 的事件并重建成 `TraceLog`；乱序文件由 `TraceLog` 按 seq 归位。
    pub fn load_log(&self, trace_id: &str) -> Result<TraceLog, Error> {
        let mut log = TraceLog::new(trace_id);
        for event in self.load()? {
            if event.trace_id == trace_id {
                log.append(event)?;
            }
        }
        Ok(log)
    }

    /// 把旧 `TraceStep` 迁移成四条完整状态事件并追加。
    pub fn append_step(
        &self,
        trace_id: &str,
        base_seq: u64,
        step: &TraceStep,
        observed_at: u64,
    ) -> Result<(), Error> {
        for event in events_from_step(trace_id, base_seq, step, observed_at) {
            self.append(&event)?;
        }
        Ok(())
    }

    fn io(&self, error: std::io::Error) -> Error {
        Error::TraceIo {
            path: self.path.display().to_string(),
            message: error.to_string(),
        }
    }
}

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
