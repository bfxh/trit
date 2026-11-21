//! 轨迹事件的 JSONL 持久化，以及旧 `TraceStep` 到事件流的迁移。
//!
//! 可选 **ECC 帧封装**（`--ecc` 语义的 API 开关，默认关闭 = 现状字节级零
//! 变化）：开启后每行经 [`trit_ops::encode_line`] 包进 GF(3) 纠错块再落盘，
//! 读路径 [`trit_ops::decode_line`] 做伴随式检查——块内单错自愈打 WARN、
//! 双错 fail-closed 拒（[`Error::TraceEcc`]）。写入机制保持与本文件既有
//! `writeln!` + `sync_data` 全量落盘语义一致。帧行自带码 tag（`TRE1 G11 …`），
//! 未开 ECC 的存储读到 ECC 行也能按 tag 译码。

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use trit_ops::{FrameCode, FrameResult, decode_line, encode_line};

use crate::trace::{TraceEvent, TraceEventKind, TraceLog, TracePayload};
use crate::{Error, TraceStep};

/// ECC 帧行前缀（[`trit_ops::frame`] 的行格式）。
const ECC_MAGIC: &str = "TRE1";

/// 一行一个 `TraceEvent` 的追加式存储。
///
/// 单次 append 是**一次原子 write**：追加模式下 Windows（`FILE_APPEND_DATA`）与 POSIX
/// （`O_APPEND`）都保证每次 write 挂到当前文件尾，多线程并发追加不会交错出半行——
/// 但调用顺序不定义事件先后，重建日志时由 `TraceLog` 按 `seq` 归位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEventStore {
    path: PathBuf,
    ecc: Option<FrameCode>,
}

impl TraceEventStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ecc: None,
        }
    }

    /// 开启 ECC 帧封装（默认块码 Golay `[11,6,5]`：单错自愈 / 双错拒绝均为
    /// 确定语义）。默认构造 [`TraceEventStore::new`] 保持字节级不变。
    pub fn with_ecc(path: impl Into<PathBuf>) -> Self {
        Self::with_ecc_code(path, FrameCode::default())
    }

    /// 开启 ECC 并指定块码（`Hamming13`/`Hamming41` 码率更高、双错语义弱一档）。
    pub fn with_ecc_code(path: impl Into<PathBuf>, code: FrameCode) -> Self {
        Self {
            path: path.into(),
            ecc: Some(code),
        }
    }

    /// 当前 ECC 设置（`None` = 关闭）。
    pub fn ecc(&self) -> Option<FrameCode> {
        self.ecc
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
        let line = match self.ecc {
            Some(code) => encode_line(code, line.as_bytes()),
            None => line,
        };
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
        // 一行一次写全（JSON 里不会有裸换行）；拆成多次 write 会让并发追加交错出半行。
        let mut bytes = line.into_bytes();
        bytes.push(b'\n');
        file.write_all(&bytes).map_err(|error| self.io(error))?;
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
            // ECC 行自带码 tag：无论本存储是否开 ECC，都按 tag 译码校验。
            let payload: std::borrow::Cow<str> = if line.starts_with(ECC_MAGIC) {
                match decode_line(line) {
                    FrameResult::Clean(bytes) => String::from_utf8(bytes)
                        .map_err(|error| self.ecc_rejected(index, &error.to_string()))?
                        .into(),
                    FrameResult::Healed { payload, repairs } => {
                        // 单错自愈：数据可证明恢复（块级 d≥3/5 纠错），但必须出声。
                        eprintln!(
                            "[ecc] {}:{}: 校验子命中，已自愈（纠正 {repairs} 个块）",
                            self.path.display(),
                            index + 1
                        );
                        String::from_utf8(payload)
                            .map_err(|error| self.ecc_rejected(index, &error.to_string()))?
                            .into()
                    }
                    FrameResult::Rejected(reason) => {
                        return Err(self.ecc_rejected(index, &reason.to_string()));
                    }
                }
            } else {
                line.into()
            };
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

    fn ecc_rejected(&self, index: usize, reason: &str) -> Error {
        Error::TraceEcc {
            path: self.path.display().to_string(),
            line: index + 1,
            reason: reason.to_string(),
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
