//! 轨迹事件的 JSONL 持久化，以及旧 `TraceStep` 到事件流的迁移。
//!
//! 可选 **ECC 帧封装**（`--ecc` 语义的 API 开关，默认关闭 = 现状字节级零
//! 变化）：开启后每行经 [`trit_ops::encode_line`] 包进 GF(3) 纠错块再落盘，
//! 读路径 [`trit_ops::decode_line`] 做伴随式检查——块内单错自愈打 WARN、
//! 双错 fail-closed 拒（[`Error::TraceEcc`]）。写入机制保持与本文件既有
//! `writeln!` + `sync_data` 全量落盘语义一致。帧行自带码 tag（`TRE1 G11 …`），
//! 未开 ECC 的存储读到 ECC 行也能按 tag 译码。

use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use trit_ops::{FrameCode, FrameResult, decode_line, encode_line};

use crate::trace::{TraceEvent, TraceEventKind, TraceLog, TracePayload};
use crate::{Error, TraceStep};

/// ECC 帧行前缀（[`trit_ops::frame`] 的行格式）。
const ECC_MAGIC: &str = "TRE1";

/// 单个轨迹文件允许的最大字节数；超过即拒绝，避免不可信输入耗尽内存。
pub const MAX_TRACE_BYTES: u64 = 256 * 1024 * 1024;

/// 追加串行化：读尾、链接、写盘、记水位必须是一个临界区。
/// 这是进程内锁；跨进程并发仍需宿主自己串行化（见模块文档）。
static APPEND_LOCK: Mutex<()> = Mutex::new(());

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
        // 读尾、链接、写盘、记水位是一个临界区；否则两个并发追加会读到同一
        // 个尾，链分叉。
        let _guard = APPEND_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (last_chain, previous) = self.last_tail()?;
        let mut sealed = event.clone();
        sealed.chain_seq = last_chain.saturating_add(1);
        sealed.seal(&previous);
        let line = serde_json::to_string(&sealed).map_err(|error| Error::TraceParse {
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
        file.sync_data().map_err(|error| self.io(error))?;
        // 水位给“尾部被截断”一个可检测的下界；它自己也是文件，删除它属于
        // 有意的反审计行为（需要外部锚点才能完全防）。
        self.write_watermark(sealed.seq)
    }

    pub fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        let metadata = match fs::metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
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
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
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
            // 存储层的每条事件都必须带内容摘要；空摘要只允许存在于未落盘的
            // 内存事件里。否则“清空 hash”就是现成的绕过口。
            if event.payload_hash.is_empty() {
                return Err(Error::TraceHashMissing {
                    path: self.path.display().to_string(),
                    line: index + 1,
                });
            }
            if event.payload_hash != event.computed_payload_hash() {
                return Err(Error::TraceHashMismatch {
                    path: self.path.display().to_string(),
                    line: index + 1,
                });
            }
            events.push(event);
        }
        self.verify_chain(&events)?;
        if let Some(watermark) = self.read_watermark()? {
            let found = events.iter().map(|event| event.seq).max().unwrap_or(0);
            if found < watermark {
                return Err(Error::TraceTruncated {
                    path: self.path.display().to_string(),
                    watermark,
                    found,
                });
            }
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

    fn watermark_path(&self) -> PathBuf {
        let mut name = self.path.as_os_str().to_os_string();
        name.push(".watermark");
        PathBuf::from(name)
    }

    fn write_watermark(&self, seq: u64) -> Result<(), Error> {
        let path = self.watermark_path();
        fs::write(&path, format!("{seq}\n")).map_err(|error| Error::TraceIo {
            path: path.display().to_string(),
            message: error.to_string(),
        })
    }

    fn read_watermark(&self) -> Result<Option<u64>, Error> {
        let path = self.watermark_path();
        match fs::read_to_string(&path) {
            Ok(text) => text
                .trim()
                .parse::<u64>()
                .map(Some)
                .map_err(|_| Error::TraceWatermark {
                    path: path.display().to_string(),
                    message: format!("bad watermark {text:?}"),
                }),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Error::TraceIo {
                path: path.display().to_string(),
                message: error.to_string(),
            }),
        }
    }

    fn verify_chain(&self, events: &[TraceEvent]) -> Result<(), Error> {
        let mut ordered: Vec<&TraceEvent> = events.iter().collect();
        // 存储层要求写入序；chain_seq=0 只允许存在于未落盘的内存事件。
        if let Some(event) = ordered.iter().find(|event| event.chain_seq == 0) {
            return Err(Error::TraceChainOrder {
                path: self.path.display().to_string(),
                chain_seq: event.chain_seq,
            });
        }
        ordered.sort_by_key(|event| event.chain_seq);
        let mut previous: Option<&TraceEvent> = None;
        let mut expected = 1u64;
        for event in ordered {
            if event.chain_seq != expected {
                return Err(Error::TraceChainOrder {
                    path: self.path.display().to_string(),
                    chain_seq: event.chain_seq,
                });
            }
            match previous {
                None => {
                    if !event.prev_hash.is_empty() {
                        return Err(Error::TraceChainBroken {
                            path: self.path.display().to_string(),
                            seq: event.seq,
                        });
                    }
                }
                Some(prior) => {
                    if event.prev_hash != prior.payload_hash {
                        return Err(Error::TraceChainBroken {
                            path: self.path.display().to_string(),
                            seq: event.seq,
                        });
                    }
                }
            }
            expected = expected.saturating_add(1);
            previous = Some(event);
        }
        Ok(())
    }

    fn decode_stored_line<'a>(
        &self,
        index: usize,
        line: &'a str,
    ) -> Result<std::borrow::Cow<'a, str>, Error> {
        // ECC 行自带码 tag：无论本存储是否开 ECC，都按 tag 译码校验。
        if !line.starts_with(ECC_MAGIC) {
            return Ok(line.into());
        }
        match decode_line(line) {
            FrameResult::Clean(bytes) => Ok(String::from_utf8(bytes)
                .map_err(|error| self.ecc_rejected(index, &error.to_string()))?
                .into()),
            FrameResult::Healed { payload, repairs } => {
                // 单错自愈：数据可证明恢复（块级 d≥3/5 纠错），但必须出声。
                eprintln!(
                    "[ecc] {}:{}: 校验子命中，已自愈（纠正 {repairs} 个块）",
                    self.path.display(),
                    index + 1
                );
                Ok(String::from_utf8(payload)
                    .map_err(|error| self.ecc_rejected(index, &error.to_string()))?
                    .into())
            }
            FrameResult::Rejected(reason) => Err(self.ecc_rejected(index, &reason.to_string())),
        }
    }

    /// 反向读文件最后一条非空事件，取它的 `(chain_seq, payload_hash)` 作为
    /// 本条的链接点。只读尾部块，避免为了追加一条事件全量加载十万条历史。
    fn last_tail(&self) -> Result<(u64, String), Error> {
        let mut file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return Ok((0, String::new()));
            }
            Err(error) => return Err(self.io(error)),
        };
        let len = file.metadata().map_err(|error| self.io(error))?.len();
        if len == 0 {
            return Ok((0, String::new()));
        }
        let mut cursor = len;
        let mut tail: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let take = usize::try_from(cursor.min(chunk.len() as u64)).unwrap_or(chunk.len());
            cursor -= take as u64;
            file.seek(SeekFrom::Start(cursor))
                .map_err(|error| self.io(error))?;
            file.read_exact(&mut chunk[..take])
                .map_err(|error| self.io(error))?;
            let mut merged = Vec::with_capacity(take + tail.len());
            merged.extend_from_slice(&chunk[..take]);
            merged.extend_from_slice(&tail);
            tail = merged;
            if cursor > 0 && !tail.contains(&b'\n') {
                continue;
            }
            for raw in tail.rsplit(|byte| *byte == b'\n') {
                let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
                if raw.iter().all(|byte| byte.is_ascii_whitespace()) {
                    continue;
                }
                let text = std::str::from_utf8(raw)
                    .map_err(|error| self.io(std::io::Error::new(ErrorKind::InvalidData, error)))?;
                let text = self.decode_stored_line(0, text)?;
                let event: TraceEvent =
                    serde_json::from_str(&text).map_err(|error| Error::TraceParse {
                        path: self.path.display().to_string(),
                        line: 0,
                        message: error.to_string(),
                    })?;
                return Ok((event.chain_seq, event.payload_hash));
            }
            if cursor == 0 {
                return Ok((0, String::new()));
            }
        }
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
