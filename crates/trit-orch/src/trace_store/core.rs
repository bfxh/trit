//! 轨迹事件的 JSONL 持久化，以及旧 `TraceStep` 到事件流的迁移。
//!
//! 可选 **ECC 帧封装**（`--ecc` 语义的 API 开关，默认关闭 = 现状字节级零
//! 变化）：开启后每行经 [`trit_ops::encode_line`] 包进 GF(3) 纠错块再落盘，
//! 读路径 [`trit_ops::decode_line`] 做伴随式检查——块内单错自愈打 WARN、
//! 双错 fail-closed 拒（[`Error::TraceEcc`]）。写入机制保持与本文件既有
//! `writeln!` + `sync_data` 全量落盘语义一致。帧行自带码 tag（`TRE1 G11 …`），
//! 未开 ECC 的存储读到 ECC 行也能按 tag 译码。

use std::fs;
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::path::PathBuf;

use trit_ops::{FrameCode, FrameResult, decode_line};

use crate::trace::{TraceEvent, TraceLog};
use crate::{Error, TraceStep};

#[path = "accessors.rs"]
mod accessors;
#[path = "anchor.rs"]
mod anchor;
#[path = "legacy.rs"]
mod legacy;
#[path = "time.rs"]
mod time;
#[path = "watermark.rs"]
mod watermark;
#[path = "write.rs"]
mod write;

pub use anchor::TraceAnchor;
pub use legacy::events_from_step;

/// ECC 帧行前缀（[`trit_ops::frame`] 的行格式）。
const ECC_MAGIC: &str = "TRE1";

/// 单个轨迹文件允许的最大字节数；超过即拒绝，避免不可信输入耗尽内存。
pub const MAX_TRACE_BYTES: u64 = 256 * 1024 * 1024;

/// 一行一个 `TraceEvent` 的追加式存储。
///
/// 单次 append 是**一次原子 write**：追加模式下 Windows（`FILE_APPEND_DATA`）与 POSIX
/// （`O_APPEND`）都保证每次 write 挂到当前文件尾，多线程并发追加不会交错出半行——
/// 但调用顺序不定义事件先后，重建日志时由 `TraceLog` 按 `seq` 归位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEventStore {
    path: PathBuf,
    ecc: Option<FrameCode>,
    strict: bool,
    anchor: Option<TraceAnchor>,
    stamp: bool,
}

impl TraceEventStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ecc: None,
            strict: false,
            anchor: None,
            stamp: false,
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
            strict: false,
            anchor: None,
            stamp: false,
        }
    }

    /// 严格模式：拒绝没有内容摘要的历史文件（默认构造会自动迁移旧格式）。
    pub fn with_strict_hash(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ecc: None,
            strict: true,
            anchor: None,
            stamp: false,
        }
    }

    /// 链头锚点（HMAC）。锚点文件缺失时退化为无锚点；严格模式用
    /// [`Self::with_anchor_required`]。
    pub fn with_anchor(path: impl Into<PathBuf>, key: impl AsRef<[u8]>) -> Self {
        let path = path.into();
        Self {
            path: path.clone(),
            ecc: None,
            strict: false,
            anchor: Some(TraceAnchor::new(path, key.as_ref().to_vec())),
            stamp: false,
        }
    }

    /// 严格锚点：非空日志没有锚点文件时直接拒绝。
    pub fn with_anchor_required(path: impl Into<PathBuf>, key: impl AsRef<[u8]>) -> Self {
        let path = path.into();
        Self {
            path: path.clone(),
            ecc: None,
            strict: false,
            anchor: Some(TraceAnchor::required(path, key.as_ref().to_vec())),
            stamp: false,
        }
    }

    pub fn append(&self, event: &TraceEvent) -> Result<(), Error> {
        self.append_batch(std::slice::from_ref(event))
    }

    pub fn load(&self) -> Result<Vec<TraceEvent>, Error> {
        let mut events = self.read_events()?;
        let mut is_legacy = false;
        for event in &events {
            if event.payload_hash.is_empty() {
                is_legacy = true;
            } else if event.payload_hash != event.computed_payload_hash() {
                return Err(Error::TraceHashMismatch {
                    path: self.path.display().to_string(),
                    line: 0,
                });
            }
        }
        if is_legacy {
            // 混合格式不是旧文件：一部分有摘要、一部分没有，按篡改拒绝。
            if events.iter().any(|event| !event.payload_hash.is_empty()) || self.strict {
                return Err(Error::TraceHashMissing {
                    path: self.path.display().to_string(),
                    line: 0,
                });
            }
            events = legacy::migrate_in_memory(events);
        } else {
            self.verify_chain(&events)?;
        }
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
        self.anchor_verify(&events)?;
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
        self.append_batch(&events_from_step(trace_id, base_seq, step, observed_at))
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
