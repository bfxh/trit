#![forbid(unsafe_code)]
//! TRWL write-ahead log v0.
//!
//! The file starts with a 32-byte segment header, then a sequence of 32-byte
//! record headers followed by payloads. When the active segment reaches its
//! size limit, the writer appends another segment header to the same file
//! (see [`DEFAULT_SEGMENT_LIMIT`]); readers accept any number of segment
//! headers and LSNs stay strictly increasing across them.
//!
//! Segment header:
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `TRWS` |
//! | 4 | 1 | version (`0`) |
//! | 5 | 3 | reserved (must be `0`) |
//! | 8 | 8 | first LSN |
//! | 16 | 12 | reserved (must be `0`) |
//! | 28 | 4 | CRC32 (IEEE) over header `0..28` |
//!
//! Record header:
//!
//! | offset | size | field |
//! |---|---|---|
//! | 0 | 4 | magic `TRWL` |
//! | 4 | 1 | version (`0`) |
//! | 5 | 1 | kind (`1` begin, `2` put, `3` delete, `4` commit, `5` abort) |
//! | 6 | 2 | flags (v0 must be `0`) |
//! | 8 | 8 | LSN (strictly increasing) |
//! | 16 | 8 | transaction id |
//! | 24 | 4 | payload length |
//! | 28 | 4 | CRC32 (IEEE) over header `0..28` + payload |
//! | 32 | … | payload |
//!
//! A record is appended with two writes (header, payload). A process crash may
//! leave a partial trailing record; [`WalReader`] reports it as
//! [`TailState::Torn`], and [`WalWriter::open`] truncates it before appending.
//! A *complete* record with a bad CRC or bad magic is corruption, not a torn
//! tail, and is rejected.
//!
//! CRC is corruption detection only, not a security mechanism. Parsing never
//! panics on arbitrary bytes.

use std::error::Error;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use tritdb_page::crc32_parts;

/// Segment magic bytes at offset 0.
pub const SEGMENT_MAGIC: [u8; 4] = *b"TRWS";
/// Segment format version.
pub const SEGMENT_VERSION: u8 = 0;
/// Segment header size in bytes.
pub const SEGMENT_HEADER_SIZE: usize = 32;

/// Record magic bytes at offset 0.
pub const RECORD_MAGIC: [u8; 4] = *b"TRWL";
/// Record format version.
pub const RECORD_VERSION: u8 = 0;
/// Record header size in bytes.
pub const RECORD_HEADER_SIZE: usize = 32;
/// Maximum record payload in bytes.
pub const MAX_RECORD_PAYLOAD: usize = 64 * 1024 * 1024;
/// Upper bound on the time index file size; a hostile file must not be
/// slurped into memory.
pub const MAX_TIME_INDEX_BYTES: u64 = 64 * 1024 * 1024;

/// Logical record type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    /// Transaction begins.
    Begin = 1,
    /// Key/value write.
    Put = 2,
    /// Key delete.
    Delete = 3,
    /// Transaction committed.
    Commit = 4,
    /// Transaction aborted.
    Abort = 5,
}

impl RecordKind {
    /// Numeric code stored in the record header.
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Stable short name.
    pub const fn name(self) -> &'static str {
        match self {
            RecordKind::Begin => "begin",
            RecordKind::Put => "put",
            RecordKind::Delete => "delete",
            RecordKind::Commit => "commit",
            RecordKind::Abort => "abort",
        }
    }

    /// Parses a numeric code.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            1 => Some(RecordKind::Begin),
            2 => Some(RecordKind::Put),
            3 => Some(RecordKind::Delete),
            4 => Some(RecordKind::Commit),
            5 => Some(RecordKind::Abort),
            _ => None,
        }
    }
}

/// Structured WAL errors.
#[derive(Debug)]
pub enum WalError {
    /// Underlying I/O error.
    Io(std::io::Error),
    /// Another process holds `<wal>.lock`.
    Locked {
        /// PID recorded in the lock file; `0` when the file could not be read.
        pid: u32,
    },
    /// Segment magic is not `TRWS`.
    BadSegmentMagic {
        /// Bytes that were found.
        found: [u8; 4],
    },
    /// Unsupported segment version.
    BadSegmentVersion {
        /// Version that was found.
        found: u8,
    },
    /// Segment header is too short or has non-zero reserved bytes.
    BadSegmentHeader {
        /// Human-readable reason.
        reason: &'static str,
    },
    /// Record magic is not `TRWL`.
    BadRecordMagic {
        /// Byte offset of the record.
        offset: u64,
        /// Bytes that were found.
        found: [u8; 4],
    },
    /// Unsupported record version.
    BadRecordVersion {
        /// Byte offset of the record.
        offset: u64,
        /// Version that was found.
        found: u8,
    },
    /// Unknown record kind.
    BadRecordKind {
        /// Byte offset of the record.
        offset: u64,
        /// Kind code that was found.
        found: u8,
    },
    /// Reserved record flags are non-zero.
    BadFlags {
        /// Byte offset of the record.
        offset: u64,
        /// Flags that were found.
        found: u16,
    },
    /// Record payload exceeds the v0 limit.
    PayloadTooLarge {
        /// Declared payload length.
        len: usize,
        /// Maximum allowed payload length.
        max: usize,
    },
    /// LSN sequence is not strictly increasing.
    LsnNotMonotonic {
        /// Previous valid LSN.
        previous: u64,
        /// LSN that was found.
        found: u64,
    },
    /// Record CRC does not match.
    ChecksumMismatch {
        /// Byte offset of the record.
        offset: u64,
        /// Stored CRC.
        expected: u32,
        /// Computed CRC.
        actual: u32,
    },
}

impl fmt::Display for WalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WalError::Io(error) => write!(f, "wal io error: {error}"),
            WalError::Locked { pid } => write!(f, "wal is locked by process {pid}"),
            WalError::BadSegmentMagic { found } => write!(
                f,
                "bad wal segment magic: found {found:?}, expected {SEGMENT_MAGIC:?}"
            ),
            WalError::BadSegmentVersion { found } => write!(
                f,
                "bad wal segment version: found {found}, expected {SEGMENT_VERSION}"
            ),
            WalError::BadSegmentHeader { reason } => {
                write!(f, "bad wal segment header: {reason}")
            }
            WalError::BadRecordMagic { offset, found } => write!(
                f,
                "bad wal record magic at {offset}: found {found:?}, expected {RECORD_MAGIC:?}"
            ),
            WalError::BadRecordVersion { offset, found } => write!(
                f,
                "bad wal record version at {offset}: found {found}, expected {RECORD_VERSION}"
            ),
            WalError::BadRecordKind { offset, found } => {
                write!(f, "bad wal record kind at {offset}: {found}")
            }
            WalError::BadFlags { offset, found } => {
                write!(f, "bad wal record flags at {offset}: {found}")
            }
            WalError::PayloadTooLarge { len, max } => {
                write!(f, "wal record payload too large: {len} bytes, max {max}")
            }
            WalError::LsnNotMonotonic { previous, found } => write!(
                f,
                "wal LSN not monotonic: previous {previous}, found {found}"
            ),
            WalError::ChecksumMismatch {
                offset,
                expected,
                actual,
            } => write!(
                f,
                "wal checksum mismatch at {offset}: stored {expected:#010x}, computed {actual:#010x}"
            ),
        }
    }
}

impl Error for WalError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WalError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for WalError {
    fn from(error: std::io::Error) -> Self {
        WalError::Io(error)
    }
}

/// State of the bytes after the last valid record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TailState {
    /// File ends exactly after a valid record.
    Clean,
    /// A trailing record header or payload is incomplete.
    Torn {
        /// Byte offset where the torn record starts.
        offset: u64,
        /// Number of bytes present for that record.
        bytes: u64,
    },
}

/// One decoded WAL record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Log sequence number.
    pub lsn: u64,
    /// Transaction id.
    pub txn_id: u64,
    /// Record kind.
    pub kind: RecordKind,
    /// Raw payload.
    pub payload: Vec<u8>,
}

/// Result of scanning a WAL file.
#[derive(Clone, Debug)]
pub struct WalScan {
    /// First LSN declared by the segment header.
    pub first_lsn: u64,
    /// Valid records in LSN order.
    pub records: Vec<Record>,
    /// Byte offset after the last valid record.
    pub valid_end: u64,
    /// Whether the trailing bytes are clean or torn.
    pub tail: TailState,
    /// Original file length at scan time.
    pub file_len: u64,
    /// Number of segment headers seen (at least one).
    pub segments: usize,
    /// Byte offset of the last segment header.
    pub last_segment_offset: u64,
    /// `first_lsn` declared by the last segment header.
    pub last_segment_first_lsn: u64,
}

impl WalScan {
    /// Last valid LSN, or `0` for an empty WAL.
    pub fn last_lsn(&self) -> u64 {
        self.records.last().map_or(0, |record| record.lsn)
    }
}

/// Time index magic bytes at offset 0.
pub const TIME_INDEX_MAGIC: [u8; 4] = *b"TRTI";
/// Time index format version.
pub const TIME_INDEX_VERSION: u8 = 0;
/// Time index header size in bytes.
pub const TIME_INDEX_HEADER_SIZE: usize = 8;
/// One time index entry: LSN (u64 LE) + wall-clock milliseconds (u64 LE).
pub const TIME_INDEX_ENTRY_SIZE: usize = 16;

/// Returns `<wal>.timeidx`.
pub fn time_index_path_for(wal_path: &Path) -> std::path::PathBuf {
    let mut name = wal_path.as_os_str().to_os_string();
    name.push(".timeidx");
    std::path::PathBuf::from(name)
}

/// Sidecar mapping wall-clock milliseconds to WAL LSNs.
///
/// The WAL record format has no timestamp field, so this index is written at
/// append time and cannot be rebuilt from the WAL. It is best-effort: a crash
/// can leave it one or more records behind, and a missing index means
/// time-based recovery is unavailable rather than silently wrong.
pub struct TimeIndex {
    file: File,
    last_lsn: u64,
    last_ts_ms: u64,
}

impl TimeIndex {
    /// Opens or creates `<wal>.timeidx`.
    pub fn open(wal_path: &Path) -> Result<Self, WalError> {
        let path = time_index_path_for(wal_path);
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        let len = file.metadata()?.len();

        if len < TIME_INDEX_HEADER_SIZE as u64 {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            write_time_index_header(&mut file)?;
            file.sync_all()?;
            return Ok(Self {
                file,
                last_lsn: 0,
                last_ts_ms: 0,
            });
        }

        let mut header = [0u8; TIME_INDEX_HEADER_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut header)?;
        if header[0..4] != TIME_INDEX_MAGIC || header[4] != TIME_INDEX_VERSION {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            write_time_index_header(&mut file)?;
            file.sync_all()?;
            return Ok(Self {
                file,
                last_lsn: 0,
                last_ts_ms: 0,
            });
        }

        if len > MAX_TIME_INDEX_BYTES {
            // A corrupt or hostile index is rebuilt from the WAL instead of
            // being read into memory.
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            write_time_index_header(&mut file)?;
            file.sync_all()?;
            return Ok(Self {
                file,
                last_lsn: 0,
                last_ts_ms: 0,
            });
        }
        file.seek(SeekFrom::Start(TIME_INDEX_HEADER_SIZE as u64))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let mut offset = 0usize;
        let mut last_lsn = 0u64;
        let mut last_ts_ms = 0u64;
        while offset + TIME_INDEX_ENTRY_SIZE <= bytes.len() {
            let lsn = u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]);
            let ts_ms = u64::from_le_bytes([
                bytes[offset + 8],
                bytes[offset + 9],
                bytes[offset + 10],
                bytes[offset + 11],
                bytes[offset + 12],
                bytes[offset + 13],
                bytes[offset + 14],
                bytes[offset + 15],
            ]);
            if lsn <= last_lsn {
                break;
            }
            last_lsn = lsn;
            last_ts_ms = last_ts_ms.max(ts_ms);
            offset += TIME_INDEX_ENTRY_SIZE;
        }
        let valid_len = TIME_INDEX_HEADER_SIZE as u64 + offset as u64;
        if valid_len < len {
            file.set_len(valid_len)?;
        }
        file.seek(SeekFrom::Start(valid_len))?;
        Ok(Self {
            file,
            last_lsn,
            last_ts_ms,
        })
    }

    /// Appends `(lsn, ts_ms)` when `lsn` moves forward. Timestamps are clamped
    /// to be non-decreasing so a clock adjustment cannot break lookups.
    pub fn append(&mut self, lsn: u64, ts_ms: u64) -> Result<(), WalError> {
        if lsn <= self.last_lsn {
            return Ok(());
        }
        let ts_ms = ts_ms.max(self.last_ts_ms);
        let mut entry = [0u8; TIME_INDEX_ENTRY_SIZE];
        entry[0..8].copy_from_slice(&lsn.to_le_bytes());
        entry[8..16].copy_from_slice(&ts_ms.to_le_bytes());
        self.file.write_all(&entry)?;
        self.last_lsn = lsn;
        self.last_ts_ms = ts_ms;
        Ok(())
    }

    /// Last indexed LSN (`0` when empty).
    pub const fn last_lsn(&self) -> u64 {
        self.last_lsn
    }

    /// Discards every entry and writes a fresh header.
    pub fn reset(&mut self) -> Result<(), WalError> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        write_time_index_header(&mut self.file)?;
        self.file.sync_all()?;
        self.last_lsn = 0;
        self.last_ts_ms = 0;
        Ok(())
    }
}

/// Looks up the largest LSN whose timestamp is at or before `ts_ms`.
///
/// Returns `Ok(None)` when the index is missing, empty, or newer than the
/// target. A missing index is not an error here; callers decide whether time
/// recovery is required.
pub fn lookup_time(wal_path: &Path, ts_ms: u64) -> Result<Option<u64>, WalError> {
    let path = time_index_path_for(wal_path);
    let mut file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if file.metadata()?.len() > MAX_TIME_INDEX_BYTES {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() < TIME_INDEX_HEADER_SIZE
        || bytes[0..4] != TIME_INDEX_MAGIC
        || bytes[4] != TIME_INDEX_VERSION
    {
        return Ok(None);
    }
    let mut best = None;
    let mut offset = TIME_INDEX_HEADER_SIZE;
    while offset + TIME_INDEX_ENTRY_SIZE <= bytes.len() {
        let lsn = u64::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
            bytes[offset + 5],
            bytes[offset + 6],
            bytes[offset + 7],
        ]);
        let ts = u64::from_le_bytes([
            bytes[offset + 8],
            bytes[offset + 9],
            bytes[offset + 10],
            bytes[offset + 11],
            bytes[offset + 12],
            bytes[offset + 13],
            bytes[offset + 14],
            bytes[offset + 15],
        ]);
        if ts <= ts_ms {
            best = Some(lsn);
        } else {
            break;
        }
        offset += TIME_INDEX_ENTRY_SIZE;
    }
    Ok(best)
}

fn write_time_index_header(file: &mut File) -> Result<(), WalError> {
    let mut header = [0u8; TIME_INDEX_HEADER_SIZE];
    header[0..4].copy_from_slice(&TIME_INDEX_MAGIC);
    header[4] = TIME_INDEX_VERSION;
    file.write_all(&header)?;
    Ok(())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Append-only WAL writer.
///
/// A writer holds an exclusive OS handle only in the sense that it is the
/// intended single writer; there is no cross-process lock in M0. Opening a
/// second writer on the same path is undefined.
fn lock_path(wal_path: &Path) -> PathBuf {
    let mut name = wal_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn process_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Cross-process writer lock: `<wal>.lock` holds the owner PID.
///
/// `create_new` makes acquisition atomic. A stale file (owner no longer alive)
/// is removed and retried once.
struct ProcessLock {
    path: PathBuf,
}

impl ProcessLock {
    fn acquire(wal_path: &Path) -> Result<Self, WalError> {
        let path = lock_path(wal_path);
        for attempt in 0..2 {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    let owner = std::fs::read_to_string(&path)
                        .ok()
                        .and_then(|text| text.trim().parse::<u32>().ok());
                    if let Some(owner) = owner {
                        if process_alive(owner) {
                            return Err(WalError::Locked { pid: owner });
                        }
                    }
                    let _ = std::fs::remove_file(&path);
                    if attempt == 1 {
                        return Err(WalError::Locked { pid: 0 });
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        Err(WalError::Locked { pid: 0 })
    }
}

impl Drop for ProcessLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct WalWriter {
    path: PathBuf,
    file: Option<File>,
    _lock: Option<ProcessLock>,
    next_lsn: u64,
    segment_first_lsn: u64,
    segment_bytes: u64,
    segment_limit: u64,
    time_index: Option<TimeIndex>,
}

/// Default size at which [`WalWriter::append`] starts a new segment.
pub const DEFAULT_SEGMENT_LIMIT: u64 = 8 * 1024 * 1024;

/// Smallest accepted segment limit. It must leave room for a header and a
/// record header, so rotation cannot loop on every append.
pub const MIN_SEGMENT_LIMIT: u64 = 1024;

/// Path of a rotated segment: `<wal>.<first_lsn>.seg`.
fn segment_path(path: &Path, first_lsn: u64) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".{first_lsn}.seg"));
    PathBuf::from(name)
}

/// Lists rotated segments as `(first_lsn, path)`, oldest first.
fn collect_rotated_segments(path: &Path) -> Result<Vec<(u64, PathBuf)>, WalError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name =
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or(WalError::BadSegmentHeader {
                reason: "wal path has no file name",
            })?;
    let prefix = format!("{file_name}.");
    let mut segments = Vec::new();
    if parent.is_dir() {
        for entry in std::fs::read_dir(parent)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(rest) = name.strip_prefix(&prefix) else {
                continue;
            };
            let Some(stem) = rest.strip_suffix(".seg") else {
                continue;
            };
            if let Ok(first_lsn) = stem.parse::<u64>() {
                segments.push((first_lsn, entry.path()));
            }
        }
    }
    segments.sort_by_key(|(first_lsn, _)| *first_lsn);
    Ok(segments)
}

struct MergedScan {
    scan: WalScan,
    active_first_lsn: u64,
    active_bytes: u64,
    has_active: bool,
}

/// Reads rotated segments plus the active segment and verifies LSN continuity.
///
/// `repair` tolerates a short active segment (left by a crash) and truncates a
/// torn tail in place. `WalReader` passes `false` and reports both as errors.
fn scan_segments(path: &Path, repair: bool) -> Result<MergedScan, WalError> {
    let rotated = collect_rotated_segments(path)?;
    let mut records = Vec::new();
    let mut first_lsn = 0u64;
    let mut previous_lsn = 0u64;
    let mut segments = 0usize;
    let mut tail = TailState::Clean;
    let mut active_valid_end = 0u64;
    let mut active_file_len = 0u64;
    let mut active_first_lsn = 0u64;
    let mut has_active = false;

    for (segment_first, segment) in &rotated {
        let bytes = std::fs::read(segment)?;
        let scan = parse_wal(&bytes)?;
        if scan.first_lsn != *segment_first {
            return Err(WalError::BadSegmentHeader {
                reason: "rotated segment name does not match its header",
            });
        }
        if first_lsn == 0 {
            first_lsn = scan.first_lsn;
        }
        if scan.first_lsn <= previous_lsn {
            return Err(WalError::LsnNotMonotonic {
                previous: previous_lsn,
                found: scan.first_lsn,
            });
        }
        for record in &scan.records {
            if record.lsn <= previous_lsn {
                return Err(WalError::LsnNotMonotonic {
                    previous: previous_lsn,
                    found: record.lsn,
                });
            }
            previous_lsn = record.lsn;
        }
        let scan_segments = scan.segments;
        records.extend(scan.records);
        segments += scan_segments;
    }

    if path.is_file() {
        let len = std::fs::metadata(path)?.len();
        if repair && len < SEGMENT_HEADER_SIZE as u64 {
            // A crash can leave a short active segment; rebuild it below.
            has_active = true;
            active_valid_end = 0;
            active_file_len = len;
            segments += 1;
        } else {
            let bytes = std::fs::read(path)?;
            let scan = parse_wal(&bytes)?;
            if first_lsn == 0 {
                first_lsn = scan.first_lsn;
            }
            if scan.first_lsn <= previous_lsn && previous_lsn > 0 {
                return Err(WalError::LsnNotMonotonic {
                    previous: previous_lsn,
                    found: scan.first_lsn,
                });
            }
            for record in &scan.records {
                if record.lsn <= previous_lsn {
                    return Err(WalError::LsnNotMonotonic {
                        previous: previous_lsn,
                        found: record.lsn,
                    });
                }
                previous_lsn = record.lsn;
            }
            let scan_segments = scan.segments;
            records.extend(scan.records);
            segments += scan_segments;
            has_active = true;
            active_first_lsn = scan.first_lsn;
            active_valid_end = scan.valid_end;
            active_file_len = scan.file_len;
            tail = scan.tail;
            if repair {
                if let TailState::Torn { offset, .. } = scan.tail {
                    let file = OpenOptions::new().write(true).open(path)?;
                    file.set_len(offset)?;
                    file.sync_all()?;
                    active_valid_end = offset;
                    active_file_len = offset;
                }
            }
        }
    }

    if segments == 0 {
        return Err(WalError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "wal does not exist",
        )));
    }
    if first_lsn == 0 {
        // Only a short active segment was present.
        first_lsn = previous_lsn.saturating_add(1).max(1);
    }

    Ok(MergedScan {
        scan: WalScan {
            first_lsn,
            records,
            valid_end: active_valid_end,
            tail,
            file_len: active_file_len,
            segments,
            last_segment_offset: 0,
            last_segment_first_lsn: active_first_lsn,
        },
        active_first_lsn,
        active_bytes: active_valid_end,
        has_active,
    })
}

impl WalWriter {
    /// Opens or creates a WAL, truncating a torn tail, and returns a scan of
    /// the bytes as they were before truncation.
    pub fn open(path: &Path) -> Result<(Self, WalScan), WalError> {
        Self::open_with_limit(path, DEFAULT_SEGMENT_LIMIT)
    }

    /// Opens or creates a WAL with an explicit segment size.
    pub fn open_with_limit(path: &Path, segment_limit: u64) -> Result<(Self, WalScan), WalError> {
        let segment_limit = segment_limit.max(MIN_SEGMENT_LIMIT);
        let lock = ProcessLock::acquire(path)?;
        let merged = match scan_segments(path, true) {
            Ok(merged) => merged,
            Err(WalError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut file = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .read(true)
                    .write(true)
                    .open(path)?;
                write_segment_header(&mut file, 1)?;
                file.sync_all()?;
                let scan = WalScan {
                    first_lsn: 1,
                    records: Vec::new(),
                    valid_end: SEGMENT_HEADER_SIZE as u64,
                    tail: TailState::Clean,
                    file_len: SEGMENT_HEADER_SIZE as u64,
                    segments: 1,
                    last_segment_offset: 0,
                    last_segment_first_lsn: 1,
                };
                return Ok((
                    Self {
                        path: path.to_path_buf(),
                        file: Some(file),
                        _lock: Some(lock),
                        next_lsn: 1,
                        segment_first_lsn: 1,
                        segment_bytes: SEGMENT_HEADER_SIZE as u64,
                        segment_limit,
                        time_index: None,
                    },
                    scan,
                ));
            }
            Err(error) => return Err(error),
        };

        let next_lsn = merged
            .scan
            .records
            .last()
            .map_or(merged.scan.first_lsn, |record| record.lsn.saturating_add(1));
        let active_first_lsn = if merged.has_active && merged.active_first_lsn > 0 {
            merged.active_first_lsn
        } else {
            next_lsn
        };

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;

        let (active_valid_end, active_bytes) = if merged.has_active && merged.active_bytes > 0 {
            (merged.active_bytes, merged.active_bytes)
        } else {
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            write_segment_header(&mut file, active_first_lsn)?;
            file.sync_all()?;
            (SEGMENT_HEADER_SIZE as u64, SEGMENT_HEADER_SIZE as u64)
        };
        file.seek(SeekFrom::Start(active_valid_end))?;

        let mut scan = merged.scan;
        scan.segments = scan.segments.max(1);
        scan.last_segment_first_lsn = active_first_lsn;
        scan.valid_end = active_valid_end;
        scan.file_len = scan.file_len.max(active_bytes);

        Ok((
            Self {
                path: path.to_path_buf(),
                file: Some(file),
                _lock: Some(lock),
                next_lsn,
                segment_first_lsn: active_first_lsn,
                segment_bytes: active_bytes,
                segment_limit,
                time_index: None,
            },
            scan,
        ))
    }

    /// Opens a WAL and keeps a `<wal>.timeidx` sidecar up to date.
    pub fn open_with_time_index(
        path: &Path,
        segment_limit: u64,
    ) -> Result<(Self, WalScan), WalError> {
        let (mut writer, scan) = Self::open_with_limit(path, segment_limit)?;
        writer.time_index = Some(TimeIndex::open(path)?);
        Ok((writer, scan))
    }

    /// Paths of every segment (rotated first, active last).
    pub fn segment_paths(&self) -> Result<Vec<PathBuf>, WalError> {
        let mut paths: Vec<PathBuf> = collect_rotated_segments(&self.path)?
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        if self.path.is_file() {
            paths.push(self.path.clone());
        }
        Ok(paths)
    }

    fn file_mut(&mut self) -> Result<&mut File, WalError> {
        self.file
            .as_mut()
            .ok_or_else(|| WalError::Io(std::io::Error::other("wal file is closed")))
    }

    /// Appends one record and optionally fsyncs it.
    pub fn append(
        &mut self,
        kind: RecordKind,
        txn_id: u64,
        payload: &[u8],
        sync: bool,
    ) -> Result<u64, WalError> {
        if payload.len() > MAX_RECORD_PAYLOAD {
            return Err(WalError::PayloadTooLarge {
                len: payload.len(),
                max: MAX_RECORD_PAYLOAD,
            });
        }

        let payload_len = u32::try_from(payload.len()).map_err(|_| WalError::PayloadTooLarge {
            len: payload.len(),
            max: MAX_RECORD_PAYLOAD,
        })?;
        let total = RECORD_HEADER_SIZE + payload.len();
        if self.segment_bytes.saturating_add(total as u64) > self.segment_limit
            && self.segment_bytes > SEGMENT_HEADER_SIZE as u64
        {
            self.rotate()?;
        }
        let lsn = self.next_lsn;

        let mut header = [0u8; RECORD_HEADER_SIZE];
        header[0..4].copy_from_slice(&RECORD_MAGIC);
        header[4] = RECORD_VERSION;
        header[5] = kind.code();
        header[6..8].copy_from_slice(&0u16.to_le_bytes());
        header[8..16].copy_from_slice(&lsn.to_le_bytes());
        header[16..24].copy_from_slice(&txn_id.to_le_bytes());
        header[24..28].copy_from_slice(&payload_len.to_le_bytes());
        let crc = crc32_parts(&header[0..28], payload);
        header[28..32].copy_from_slice(&crc.to_le_bytes());

        let file = self.file_mut()?;
        file.write_all(&header)?;
        file.write_all(payload)?;
        if sync {
            file.sync_all()?;
        }

        self.next_lsn = self.next_lsn.saturating_add(1);
        self.segment_bytes = self.segment_bytes.saturating_add(total as u64);
        if let Some(index) = &mut self.time_index {
            let _ = index.append(lsn, now_ms());
        }
        Ok(lsn)
    }

    /// Rotates the active segment into `<wal>.<first_lsn>.seg` and starts a
    /// fresh active segment.
    fn rotate(&mut self) -> Result<(), WalError> {
        if let Some(file) = self.file.as_mut() {
            file.sync_all()?;
        }
        // Windows cannot rename an open file.
        self.file = None;

        let target = segment_path(&self.path, self.segment_first_lsn);
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        std::fs::rename(&self.path, &target)?;

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&self.path)?;
        write_segment_header(&mut file, self.next_lsn)?;
        file.sync_all()?;
        self.file = Some(file);
        self.segment_first_lsn = self.next_lsn;
        self.segment_bytes = SEGMENT_HEADER_SIZE as u64;
        Ok(())
    }

    /// Flushes the file to stable storage.
    pub fn sync(&mut self) -> Result<(), WalError> {
        if let Some(file) = self.file.as_mut() {
            file.sync_all()?;
        }
        Ok(())
    }

    /// Replaces the WAL with an empty segment whose first LSN is `first_lsn`.
    ///
    /// Every rotated segment is removed as well.
    pub fn reset(&mut self, first_lsn: u64) -> Result<(), WalError> {
        if first_lsn == 0 {
            return Err(WalError::BadSegmentHeader {
                reason: "first LSN must be non-zero",
            });
        }
        if let Some(file) = self.file.as_mut() {
            file.sync_all()?;
        }
        self.file = None;

        for (_, segment) in collect_rotated_segments(&self.path)? {
            let _ = std::fs::remove_file(segment);
        }

        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&self.path)?;
        write_segment_header(&mut file, first_lsn)?;
        file.sync_all()?;
        self.file = Some(file);
        self.next_lsn = first_lsn;
        self.segment_first_lsn = first_lsn;
        self.segment_bytes = SEGMENT_HEADER_SIZE as u64;
        if let Some(index) = &mut self.time_index {
            if first_lsn <= index.last_lsn() {
                index.reset()?;
            }
        }
        Ok(())
    }

    /// LSN that the next appended record will use.
    pub const fn next_lsn(&self) -> u64 {
        self.next_lsn
    }
}

/// Read-only WAL scanner.
pub struct WalReader;

impl WalReader {
    /// Opens a WAL for reading and scans its current prefix.
    pub fn open(path: &Path) -> Result<WalScan, WalError> {
        scan_segments(path, false).map(|merged| merged.scan)
    }
}

/// Parses a complete WAL byte image.
pub fn parse_wal(bytes: &[u8]) -> Result<WalScan, WalError> {
    if bytes.len() < SEGMENT_HEADER_SIZE {
        return Err(WalError::BadSegmentHeader {
            reason: "file is shorter than the segment header",
        });
    }

    let found_magic = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if found_magic != SEGMENT_MAGIC {
        return Err(WalError::BadSegmentMagic { found: found_magic });
    }
    if bytes[4] != SEGMENT_VERSION {
        return Err(WalError::BadSegmentVersion { found: bytes[4] });
    }
    if bytes[5..8].iter().any(|byte| *byte != 0) || bytes[16..28].iter().any(|byte| *byte != 0) {
        return Err(WalError::BadSegmentHeader {
            reason: "reserved bytes must be zero",
        });
    }
    let first_lsn = read_u64(bytes, 8)?;
    if first_lsn == 0 {
        return Err(WalError::BadSegmentHeader {
            reason: "first LSN must be non-zero",
        });
    }
    let stored_crc = read_u32(bytes, 28)?;
    let computed_crc = crc32_parts(&bytes[0..28], &[]);
    if stored_crc != computed_crc {
        return Err(WalError::ChecksumMismatch {
            offset: 0,
            expected: stored_crc,
            actual: computed_crc,
        });
    }

    let mut records = Vec::new();
    let mut offset = SEGMENT_HEADER_SIZE;
    let mut previous_lsn = 0u64;
    let mut tail = TailState::Clean;
    let mut segments = 1usize;
    let mut last_segment_offset = 0u64;
    let mut last_segment_first_lsn = first_lsn;

    while offset < bytes.len() {
        let remaining = bytes.len() - offset;
        if remaining < RECORD_HEADER_SIZE {
            tail = TailState::Torn {
                offset: offset as u64,
                bytes: remaining as u64,
            };
            break;
        }

        if bytes[offset..offset + 4] == SEGMENT_MAGIC {
            if bytes[offset + 4] != SEGMENT_VERSION {
                return Err(WalError::BadSegmentVersion {
                    found: bytes[offset + 4],
                });
            }
            if bytes[offset + 5..offset + 8].iter().any(|byte| *byte != 0)
                || bytes[offset + 16..offset + 28]
                    .iter()
                    .any(|byte| *byte != 0)
            {
                return Err(WalError::BadSegmentHeader {
                    reason: "reserved bytes must be zero",
                });
            }
            let seg_first = read_u64(bytes, offset + 8)?;
            if seg_first == 0 {
                return Err(WalError::BadSegmentHeader {
                    reason: "first LSN must be non-zero",
                });
            }
            let stored_crc = read_u32(bytes, offset + 28)?;
            let computed_crc = crc32_parts(&bytes[offset..offset + 28], &[]);
            if stored_crc != computed_crc {
                return Err(WalError::ChecksumMismatch {
                    offset: offset as u64,
                    expected: stored_crc,
                    actual: computed_crc,
                });
            }
            if seg_first <= previous_lsn {
                return Err(WalError::LsnNotMonotonic {
                    previous: previous_lsn,
                    found: seg_first,
                });
            }
            segments += 1;
            last_segment_offset = offset as u64;
            last_segment_first_lsn = seg_first;
            offset += SEGMENT_HEADER_SIZE;
            continue;
        }

        let header = &bytes[offset..offset + RECORD_HEADER_SIZE];
        let found_magic = [header[0], header[1], header[2], header[3]];
        if found_magic != RECORD_MAGIC {
            return Err(WalError::BadRecordMagic {
                offset: offset as u64,
                found: found_magic,
            });
        }
        if header[4] != RECORD_VERSION {
            return Err(WalError::BadRecordVersion {
                offset: offset as u64,
                found: header[4],
            });
        }
        let kind = RecordKind::from_u8(header[5]).ok_or(WalError::BadRecordKind {
            offset: offset as u64,
            found: header[5],
        })?;
        let flags = u16::from_le_bytes([header[6], header[7]]);
        if flags != 0 {
            return Err(WalError::BadFlags {
                offset: offset as u64,
                found: flags,
            });
        }
        let lsn = read_u64(header, 8)?;
        let txn_id = read_u64(header, 16)?;
        let payload_len = read_u32(header, 24)? as usize;
        if payload_len > MAX_RECORD_PAYLOAD {
            return Err(WalError::PayloadTooLarge {
                len: payload_len,
                max: MAX_RECORD_PAYLOAD,
            });
        }
        let total = RECORD_HEADER_SIZE + payload_len;
        if remaining < total {
            tail = TailState::Torn {
                offset: offset as u64,
                bytes: remaining as u64,
            };
            break;
        }

        let payload = &bytes[offset + RECORD_HEADER_SIZE..offset + total];
        let stored_crc = read_u32(header, 28)?;
        let computed_crc = crc32_parts(&header[0..28], payload);
        if stored_crc != computed_crc {
            return Err(WalError::ChecksumMismatch {
                offset: offset as u64,
                expected: stored_crc,
                actual: computed_crc,
            });
        }
        if lsn <= previous_lsn || lsn < last_segment_first_lsn {
            return Err(WalError::LsnNotMonotonic {
                previous: previous_lsn.max(last_segment_first_lsn.saturating_sub(1)),
                found: lsn,
            });
        }

        records.push(Record {
            lsn,
            txn_id,
            kind,
            payload: payload.to_vec(),
        });
        previous_lsn = lsn;
        offset += total;
    }

    Ok(WalScan {
        first_lsn,
        records,
        valid_end: offset as u64,
        tail,
        file_len: bytes.len() as u64,
        segments,
        last_segment_offset,
        last_segment_first_lsn,
    })
}

fn write_segment_header(file: &mut File, first_lsn: u64) -> Result<(), WalError> {
    let mut header = [0u8; SEGMENT_HEADER_SIZE];
    header[0..4].copy_from_slice(&SEGMENT_MAGIC);
    header[4] = SEGMENT_VERSION;
    header[8..16].copy_from_slice(&first_lsn.to_le_bytes());
    let crc = crc32_parts(&header[0..28], &[]);
    header[28..32].copy_from_slice(&crc.to_le_bytes());
    file.write_all(&header)?;
    Ok(())
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, WalError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or(WalError::BadSegmentHeader {
            reason: "missing u32 field",
        })?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, WalError> {
    let slice = bytes
        .get(offset..offset + 8)
        .ok_or(WalError::BadSegmentHeader {
            reason: "missing u64 field",
        })?;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tritdb-wal-test-{}-{}",
                std::process::id(),
                NEXT_DIR.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).expect("create temp dir");
            Self(path)
        }

        fn wal(&self) -> PathBuf {
            self.0.join("test.wal")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn crash_during_reset_leaves_a_recoverable_wal() {
        let dir = TempDir::new();
        let path = dir.wal();

        // (1) `set_len(0)` completed and the process died: empty file.
        fs::write(&path, b"").expect("empty file");
        let (writer, scan) = WalWriter::open(&path).expect("repair empty file");
        assert!(scan.records.is_empty());
        drop(writer);

        // (2) Only the segment magic landed before the crash.
        fs::write(&path, b"TRWS").expect("partial header");
        let (writer, scan) = WalWriter::open(&path).expect("repair partial header");
        assert!(scan.records.is_empty());
        drop(writer);

        // (3) The repaired WAL accepts new records.
        let (mut writer, scan) = WalWriter::open(&path).expect("open repaired wal");
        assert!(scan.records.is_empty());
        writer
            .append(RecordKind::Begin, 1, &[], false)
            .expect("append after repair");
    }

    #[test]
    fn creates_empty_segment_and_roundtrips_records() {
        let dir = TempDir::new();
        let path = dir.wal();
        let (mut writer, scan) = WalWriter::open(&path).expect("create wal");
        assert!(scan.records.is_empty());
        assert_eq!(scan.tail, TailState::Clean);
        assert_eq!(scan.valid_end, SEGMENT_HEADER_SIZE as u64);

        let l1 = writer
            .append(RecordKind::Begin, 7, &[], false)
            .expect("begin");
        let l2 = writer
            .append(RecordKind::Put, 7, b"payload", false)
            .expect("put");
        let l3 = writer
            .append(RecordKind::Commit, 7, &[], true)
            .expect("commit");
        assert_eq!([l1, l2, l3], [1, 2, 3]);
        drop(writer);

        let scan = WalReader::open(&path).expect("read wal");
        assert_eq!(scan.records.len(), 3);
        assert_eq!(scan.records[0].kind, RecordKind::Begin);
        assert_eq!(scan.records[1].payload, b"payload");
        assert_eq!(scan.records[2].txn_id, 7);
        assert_eq!(scan.last_lsn(), 3);
        assert_eq!(scan.tail, TailState::Clean);
    }

    #[test]
    fn torn_tail_is_reported_then_truncated() {
        let dir = TempDir::new();
        let path = dir.wal();
        let (mut writer, _) = WalWriter::open(&path).expect("create wal");
        writer
            .append(RecordKind::Begin, 1, &[], false)
            .expect("begin");
        drop(writer);

        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open for append");
        file.write_all(b"TRW").expect("write torn header");
        drop(file);

        let scan = WalReader::open(&path).expect("scan torn");
        assert_eq!(
            scan.tail,
            TailState::Torn {
                offset: SEGMENT_HEADER_SIZE as u64 + RECORD_HEADER_SIZE as u64,
                bytes: 3
            }
        );

        let (mut writer, scan) = WalWriter::open(&path).expect("open and truncate");
        assert_eq!(scan.records.len(), 1);
        assert_eq!(
            scan.tail,
            TailState::Torn {
                offset: SEGMENT_HEADER_SIZE as u64 + RECORD_HEADER_SIZE as u64,
                bytes: 3
            }
        );
        assert_eq!(writer.next_lsn(), 2);
        assert_eq!(
            fs::metadata(&path).expect("metadata").len(),
            SEGMENT_HEADER_SIZE as u64 + RECORD_HEADER_SIZE as u64
        );

        let lsn = writer
            .append(RecordKind::Commit, 1, &[], true)
            .expect("append after truncate");
        assert_eq!(lsn, 2);
        drop(writer);
        let scan = WalReader::open(&path).expect("read repaired wal");
        assert_eq!(scan.records.len(), 2);
        assert_eq!(scan.tail, TailState::Clean);
    }

    #[test]
    fn reset_truncates_to_a_new_empty_segment() {
        let dir = TempDir::new();
        let path = dir.wal();
        let (mut writer, _) = WalWriter::open(&path).expect("create wal");
        writer
            .append(RecordKind::Begin, 1, &[], false)
            .expect("begin");
        writer.reset(100).expect("reset");
        assert_eq!(writer.next_lsn(), 100);
        drop(writer);

        let scan = WalReader::open(&path).expect("read reset wal");
        assert_eq!(scan.first_lsn, 100);
        assert!(scan.records.is_empty());
        assert_eq!(scan.valid_end, SEGMENT_HEADER_SIZE as u64);
        assert_eq!(scan.tail, TailState::Clean);

        let (mut writer, _) = WalWriter::open(&path).expect("reopen reset wal");
        let lsn = writer
            .append(RecordKind::Commit, 2, &[], true)
            .expect("append after reset");
        assert_eq!(lsn, 100);
    }

    #[test]
    fn open_repairs_a_short_segment_file() {
        let dir = TempDir::new();
        let path = dir.wal();
        fs::write(&path, b"TRW").expect("write short file");

        let (mut writer, scan) = WalWriter::open(&path).expect("repair short wal");
        assert_eq!(scan.first_lsn, 1);
        assert!(scan.records.is_empty());
        assert_eq!(scan.tail, TailState::Clean);
        assert_eq!(
            fs::metadata(&path).expect("metadata").len(),
            SEGMENT_HEADER_SIZE as u64
        );

        let lsn = writer
            .append(RecordKind::Begin, 9, &[], true)
            .expect("append after repair");
        assert_eq!(lsn, 1);
    }

    #[test]
    fn complete_record_with_bad_crc_is_corruption() {
        let dir = TempDir::new();
        let path = dir.wal();
        let (mut writer, _) = WalWriter::open(&path).expect("create wal");
        writer
            .append(RecordKind::Put, 1, b"abc", true)
            .expect("put");
        drop(writer);

        let mut bytes = fs::read(&path).expect("read");
        let payload_offset = SEGMENT_HEADER_SIZE + RECORD_HEADER_SIZE;
        bytes[payload_offset + 1] ^= 0xff;
        fs::write(&path, &bytes).expect("write");

        let error = WalReader::open(&path).expect_err("bad crc must fail");
        assert!(matches!(
            error,
            WalError::ChecksumMismatch {
                offset,
                ..
            } if offset == SEGMENT_HEADER_SIZE as u64
        ));
    }

    #[test]
    fn rejects_bad_segment_magic_and_version() {
        let dir = TempDir::new();
        let path = dir.wal();
        let (writer, _) = WalWriter::open(&path).expect("create wal");
        drop(writer);
        let original = fs::read(&path).expect("read");

        let mut bytes = original.clone();
        bytes[0] = b'X';
        fs::write(&path, &bytes).expect("write");
        assert!(matches!(
            WalReader::open(&path).expect_err("bad magic"),
            WalError::BadSegmentMagic { .. }
        ));

        let mut bytes = original;
        bytes[4] = 9;
        fs::write(&path, &bytes).expect("write");
        assert!(matches!(
            WalReader::open(&path).expect_err("bad version"),
            WalError::BadSegmentVersion { found: 9 }
        ));
    }

    #[test]
    fn rejects_bad_record_kind_and_flags() {
        let dir = TempDir::new();
        let path = dir.wal();
        let (mut writer, _) = WalWriter::open(&path).expect("create wal");
        writer
            .append(RecordKind::Begin, 1, &[], true)
            .expect("begin");
        drop(writer);
        let original = fs::read(&path).expect("read");
        let record_offset = SEGMENT_HEADER_SIZE;

        let mut bytes = original.clone();
        bytes[record_offset + 5] = 99;
        fs::write(&path, &bytes).expect("write");
        assert!(matches!(
            WalReader::open(&path).expect_err("bad kind"),
            WalError::BadRecordKind { found: 99, .. }
        ));

        let mut bytes = original;
        bytes[record_offset + 6] = 1;
        fs::write(&path, &bytes).expect("write");
        assert!(matches!(
            WalReader::open(&path).expect_err("bad flags"),
            WalError::BadFlags { found: 1, .. }
        ));
    }
}
