#![forbid(unsafe_code)]
//! Single-WAL-writer, multi-transaction store over [`tritdb_wal`].
//!
//! M0 model:
//!
//! - One WAL writer: every append takes the WAL mutex, but the mutex is not
//!   held for the lifetime of a transaction. Multiple logical transactions can
//!   interleave `Begin`/`Put`/`Delete` records.
//! - Many readers: [`TxnStore::get`] / [`TxnStore::iter`] take a read lock on
//!   the in-memory map and never touch the WAL.
//! - Commit appends a `Commit` record without fsync, then joins a commit
//!   queue. One leader fsyncs a batch of commits and applies them in WAL order;
//!   the other committers wait for that leader.
//! - Recovery replays WAL records and applies only transactions with a
//!   `Commit` record. A crash can leave `Begin`/`Put` records without a
//!   `Commit`; those are discarded.
//! - [`TxnStore::checkpoint`] writes the committed map to `<wal>.snap`, then
//!   resets the WAL. Recovery loads the snapshot and applies only records
//!   after the snapshot LSN. Checkpoint refuses while logical transactions are
//!   active.
//!
//! This is not MVCC. Readers see the map as of the last applied commit batch;
//! there are no snapshots, no version chains, and no concurrent conflicting
//! writes resolved by anything other than WAL order.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};

use tritdb_page::crc32_parts;
use tritdb_wal::{RecordKind, WalError, WalScan, WalWriter};

/// A committed key/value pair.
pub type Entry = (Vec<u8>, Vec<u8>);

/// Snapshot magic bytes at offset 0.
pub const SNAPSHOT_MAGIC: [u8; 4] = *b"TRSN";
/// Snapshot format version.
pub const SNAPSHOT_VERSION: u8 = 0;
/// Snapshot header size in bytes.
pub const SNAPSHOT_HEADER_SIZE: usize = 32;
/// Largest snapshot payload representable by the v0 `u32` length field.
pub const MAX_SNAPSHOT_PAYLOAD: usize = u32::MAX as usize;

/// Number of scheduler yields the leader uses to let more commits join a
/// group. This is a yield window, not a timed sleep, so a lone committer pays
/// only a few scheduling points rather than a fixed millisecond delay.
const GROUP_COMMIT_YIELDS: usize = 32;

/// Structured transaction errors.
#[derive(Debug)]
pub enum TxnError {
    /// WAL error.
    Wal(WalError),
    /// Snapshot or WAL file I/O error.
    Io(std::io::Error),
    /// Key does not fit in the WAL payload's `u32` length.
    KeyTooLarge {
        /// Key length.
        len: usize,
    },
    /// Value does not fit in the WAL payload's `u32` length.
    ValueTooLarge {
        /// Value length.
        len: usize,
    },
    /// A WAL payload could not be decoded.
    MalformedPayload {
        /// Record kind being decoded.
        kind: RecordKind,
        /// Human-readable reason.
        reason: &'static str,
    },
    /// Checkpoint was requested while logical transactions were still active.
    ActiveTxns {
        /// Active transaction count at the check.
        count: usize,
    },
    /// The commit queue failed and the store cannot accept more commits.
    Writer {
        /// Underlying error text.
        reason: String,
    },
    /// Snapshot header, payload, or CRC is invalid.
    BadSnapshot {
        /// Human-readable reason.
        reason: &'static str,
    },
    /// Snapshot payload exceeds the v0 limit.
    SnapshotTooLarge {
        /// Payload length.
        len: usize,
        /// Maximum allowed payload length.
        max: usize,
    },
    /// An operation references a transaction with no `Begin` record.
    UnknownTxn {
        /// Transaction id.
        txn_id: u64,
        /// Record kind that referenced it.
        kind: RecordKind,
    },
    /// A second `Begin` was seen for the same transaction.
    DuplicateBegin {
        /// Transaction id.
        txn_id: u64,
    },
    /// `Commit`/`Abort` has no matching `Begin`.
    MissingBegin {
        /// Transaction id.
        txn_id: u64,
    },
    /// A concurrent transaction committed a key this transaction also wrote.
    Conflict {
        /// Key that conflicted (lossy UTF-8 for display).
        key: String,
        /// LSN at which this transaction started.
        begin_lsn: u64,
        /// LSN of the last committed write to the key.
        last_lsn: u64,
    },
    /// PITR target is older than the oldest archived checkpoint.
    PitrBeforeHistory {
        /// Requested LSN.
        target_lsn: u64,
        /// Oldest checkpoint LSN available in the archive.
        oldest_lsn: u64,
    },
    /// A PITR archive is missing its snapshot or WAL half.
    PitrIncompleteArchive {
        /// Checkpoint LSN of the damaged archive.
        checkpoint_lsn: u64,
        /// Human-readable reason.
        reason: &'static str,
    },
    /// Time-based recovery was requested but the time index has no entry at
    /// or before the target.
    TimeIndexUnavailable,
    /// A lock was poisoned by a panicking thread.
    Poisoned,
}

impl fmt::Display for TxnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxnError::Wal(error) => write!(f, "transaction wal error: {error}"),
            TxnError::Io(error) => write!(f, "transaction io error: {error}"),
            TxnError::KeyTooLarge { len } => write!(f, "key too large for wal payload: {len}"),
            TxnError::ValueTooLarge { len } => {
                write!(f, "value too large for wal payload: {len}")
            }
            TxnError::MalformedPayload { kind, reason } => {
                write!(f, "malformed {} payload: {reason}", kind.name())
            }
            TxnError::ActiveTxns { count } => {
                write!(f, "checkpoint requires no active txns, found {count}")
            }
            TxnError::Writer { reason } => write!(f, "commit writer failed: {reason}"),
            TxnError::BadSnapshot { reason } => write!(f, "bad snapshot: {reason}"),
            TxnError::SnapshotTooLarge { len, max } => {
                write!(f, "snapshot payload too large: {len} bytes, max {max}")
            }
            TxnError::UnknownTxn { txn_id, kind } => {
                write!(f, "{} record for unknown txn {txn_id}", kind.name())
            }
            TxnError::DuplicateBegin { txn_id } => {
                write!(f, "duplicate begin for txn {txn_id}")
            }
            TxnError::MissingBegin { txn_id } => {
                write!(f, "commit/abort for txn {txn_id} without begin")
            }
            TxnError::Conflict {
                key,
                begin_lsn,
                last_lsn,
            } => write!(
                f,
                "write conflict on {key:?}: txn began at lsn {begin_lsn}, last write at {last_lsn}"
            ),
            TxnError::PitrBeforeHistory {
                target_lsn,
                oldest_lsn,
            } => write!(
                f,
                "point-in-time target lsn {target_lsn} predates oldest archived checkpoint {oldest_lsn}"
            ),
            TxnError::PitrIncompleteArchive {
                checkpoint_lsn,
                reason,
            } => write!(
                f,
                "pitr archive at lsn {checkpoint_lsn} is incomplete: {reason}"
            ),
            TxnError::TimeIndexUnavailable => {
                write!(f, "time index has no entry at or before the requested timestamp")
            }
            TxnError::Poisoned => write!(f, "transaction lock poisoned"),
        }
    }
}

impl Error for TxnError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            TxnError::Wal(error) => Some(error),
            TxnError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<WalError> for TxnError {
    fn from(error: WalError) -> Self {
        TxnError::Wal(error)
    }
}

impl From<std::io::Error> for TxnError {
    fn from(error: std::io::Error) -> Self {
        TxnError::Io(error)
    }
}

/// What recovery observed when opening the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryInfo {
    /// Number of valid WAL records applied after the checkpoint.
    pub records: usize,
    /// WAL records skipped because they are at or before the snapshot LSN.
    pub records_skipped: usize,
    /// LSN covered by the snapshot; `0` means no snapshot.
    pub checkpoint_lsn: u64,
    /// Number of transactions applied because they had a `Commit` record.
    pub committed_txns: usize,
    /// Number of transactions with an `Abort` record.
    pub aborted_txns: usize,
    /// Number of `Begin` transactions with no `Commit`/`Abort`; discarded.
    pub discarded_txns: usize,
    /// Bytes in the torn trailing record, if any.
    pub torn_tail_bytes: u64,
    /// Last valid LSN.
    pub last_lsn: u64,
}

/// Result of a successful commit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitInfo {
    /// Transaction id.
    pub txn_id: u64,
    /// LSN of the `Commit` record.
    pub commit_lsn: u64,
    /// Number of operations applied.
    pub op_count: usize,
}

/// Default number of archived checkpoints kept by [`TxnStore::checkpoint_archive`].
pub const DEFAULT_ARCHIVE_KEEP: usize = 8;

/// Result of a successful key/value checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointInfo {
    /// Last WAL LSN represented by the snapshot.
    pub checkpoint_lsn: u64,
    /// Number of committed keys written to the snapshot.
    pub entries: u64,
    /// WAL bytes before the reset.
    pub wal_bytes_before: u64,
    /// WAL bytes after the reset.
    pub wal_bytes_after: u64,
    /// Snapshot file size in bytes.
    pub snapshot_bytes: u64,
    /// Bytes copied into `<wal>.history/<lsn>.wal`; `None` for a plain checkpoint.
    pub archived_wal_bytes: Option<u64>,
    /// Bytes copied into `<wal>.history/<lsn>.snap`; `None` for a plain checkpoint.
    pub archived_snapshot_bytes: Option<u64>,
    /// Archived checkpoints removed by the retention policy (`0` when off).
    pub pruned_archives: usize,
}

/// State reconstructed at a requested historical LSN.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PitrView {
    /// LSN the caller asked for.
    pub target_lsn: u64,
    /// Checkpoint LSN of the snapshot used as the replay base.
    pub checkpoint_lsn: u64,
    /// Highest WAL LSN actually replayed (at most `target_lsn`).
    pub applied_lsn: u64,
    /// Committed key/value pairs at the target LSN, sorted by key.
    pub entries: Vec<Entry>,
    /// Transactions applied because their `Commit` was at or before the target.
    pub committed_txns: usize,
    /// Transactions started but not committed at the target.
    pub discarded_txns: usize,
}

/// Result of pruning archived checkpoints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PruneInfo {
    /// Checkpoint LSNs removed, oldest first.
    pub removed: Vec<u64>,
    /// Checkpoint LSNs kept, oldest first.
    pub kept: Vec<u64>,
}

/// A store with one WAL writer and concurrent logical transactions.
pub struct TxnStore {
    path: PathBuf,
    snapshot_path: PathBuf,
    wal: Mutex<WalWriter>,
    state: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
    write_lsn: RwLock<BTreeMap<Vec<u8>, u64>>,
    commit_queue: Mutex<CommitQueue>,
    commit_ready: Condvar,
    next_txn_id: AtomicU64,
    active_txns: AtomicUsize,
    syncs: AtomicU64,
    recovery: RecoveryInfo,
}

struct CommitQueue {
    next_seq: u64,
    synced_seq: u64,
    leader: Option<u64>,
    works: Vec<CommitWork>,
    failure: Option<String>,
}

struct CommitWork {
    seq: u64,
    commit_lsn: u64,
    ops: Vec<Op>,
}

impl TxnStore {
    /// Opens or creates a store and replays committed transactions.
    pub fn open(path: &Path) -> Result<Self, TxnError> {
        Self::open_with_wal_limit(path, tritdb_wal::DEFAULT_SEGMENT_LIMIT)
    }

    /// Opens a store with an explicit WAL segment size (used by tests to
    /// force multi-file rotation).
    pub fn open_with_wal_limit(path: &Path, segment_limit: u64) -> Result<Self, TxnError> {
        let snapshot_path = snapshot_path_for(path);
        let snapshot = read_snapshot(&snapshot_path)?;
        let checkpoint_lsn = snapshot.as_ref().map_or(0, |snap| snap.checkpoint_lsn);
        let state = snapshot.map_or_else(BTreeMap::new, |snap| snap.entries);

        let (wal, scan) = WalWriter::open_with_time_index(path, segment_limit)?;
        let recovered = recover(&scan, state, checkpoint_lsn)?;
        let next_txn_id = recovered.max_txn_id.saturating_add(1).max(1);

        Ok(Self {
            path: path.to_path_buf(),
            snapshot_path,
            wal: Mutex::new(wal),
            state: RwLock::new(recovered.state),
            write_lsn: RwLock::new(BTreeMap::new()),
            commit_queue: Mutex::new(CommitQueue {
                next_seq: 0,
                synced_seq: 0,
                leader: None,
                works: Vec::new(),
                failure: None,
            }),
            commit_ready: Condvar::new(),
            next_txn_id: AtomicU64::new(next_txn_id),
            active_txns: AtomicUsize::new(0),
            syncs: AtomicU64::new(0),
            recovery: recovered.info,
        })
    }

    /// WAL path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Snapshot path (`<wal>.snap`).
    pub fn snapshot_path(&self) -> &Path {
        &self.snapshot_path
    }

    /// Recovery report captured at open time.
    pub fn recovery(&self) -> &RecoveryInfo {
        &self.recovery
    }

    /// Number of fsync calls issued by commit leaders since open.
    pub fn sync_count(&self) -> u64 {
        self.syncs.load(Ordering::SeqCst)
    }

    /// Number of logical transactions that have begun but not finished.
    pub fn active_txns(&self) -> usize {
        self.active_txns.load(Ordering::SeqCst)
    }

    /// Starts a logical transaction.
    ///
    /// The WAL mutex is held only while the `Begin` record is appended. The
    /// returned [`Txn`] does not hold it, so other transactions can append and
    /// commit concurrently. `Commit` batches fsyncs under one leader.
    pub fn begin(&self) -> Result<Txn<'_>, TxnError> {
        let mut wal = self.wal.lock().map_err(|_| TxnError::Poisoned)?;
        let txn_id = self.next_txn_id.fetch_add(1, Ordering::SeqCst);
        let begin_lsn = wal.next_lsn();
        let snapshot = Arc::new(self.state.read().map_err(|_| TxnError::Poisoned)?.clone());
        self.active_txns.fetch_add(1, Ordering::SeqCst);
        if let Err(error) = wal.append(RecordKind::Begin, txn_id, &[], false) {
            self.active_txns.fetch_sub(1, Ordering::SeqCst);
            return Err(error.into());
        }
        Ok(Txn {
            store: self,
            txn_id,
            ops: Vec::new(),
            active: true,
            snapshot,
            begin_lsn,
        })
    }

    /// Reads a committed value.
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        let state = self.state.read().map_err(|_| TxnError::Poisoned)?;
        Ok(state.get(key).cloned())
    }

    /// Returns all committed key/value pairs.
    pub fn iter(&self) -> Result<Vec<Entry>, TxnError> {
        let state = self.state.read().map_err(|_| TxnError::Poisoned)?;
        Ok(state
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect())
    }

    /// Number of committed keys.
    pub fn len(&self) -> Result<usize, TxnError> {
        let state = self.state.read().map_err(|_| TxnError::Poisoned)?;
        Ok(state.len())
    }

    /// Returns `true` when there are no committed keys.
    pub fn is_empty(&self) -> Result<bool, TxnError> {
        Ok(self.len()? == 0)
    }

    /// Writes the committed map to `<wal>.snap` and resets the WAL.
    ///
    /// The snapshot is written and fsynced before the WAL reset. If a crash
    /// happens between those two steps, recovery loads the snapshot and skips
    /// the old WAL records it already covers.
    pub fn checkpoint(&self) -> Result<CheckpointInfo, TxnError> {
        self.checkpoint_impl(false, 0)
    }

    /// Like [`TxnStore::checkpoint`], but first copies the snapshot and the
    /// pre-reset WAL into `<wal>.history/<lsn>.{snap,wal}`.
    ///
    /// The archive is what [`TxnStore::recover_at`] replays. A plain
    /// checkpoint discards the WAL it reset and therefore cannot be used as a
    /// point-in-time base.
    pub fn checkpoint_archive(&self) -> Result<CheckpointInfo, TxnError> {
        self.checkpoint_impl(true, DEFAULT_ARCHIVE_KEEP)
    }

    /// Like [`TxnStore::checkpoint_archive`], but keeps only the newest `keep`
    /// archived checkpoints. `keep = 0` disables automatic pruning.
    ///
    /// Pruning runs after the new archive is written, so the newest checkpoint
    /// is always kept and a failed archive leaves the previous history intact.
    pub fn checkpoint_archive_keep(&self, keep: usize) -> Result<CheckpointInfo, TxnError> {
        self.checkpoint_impl(true, keep)
    }

    fn checkpoint_impl(&self, archive: bool, keep: usize) -> Result<CheckpointInfo, TxnError> {
        let mut wal = self.wal.lock().map_err(|_| TxnError::Poisoned)?;
        // Check under the WAL lock. A `begin` increments `active_txns` before
        // appending its `Begin`; if it increments after this check it cannot
        // append until the reset is done, so its `Begin` lands after the new
        // segment header. If it incremented before, this check sees it and
        // rejects.
        let active = self.active_txns.load(Ordering::SeqCst);
        if active != 0 {
            return Err(TxnError::ActiveTxns { count: active });
        }
        let state = self.state.read().map_err(|_| TxnError::Poisoned)?;

        let checkpoint_lsn = wal
            .next_lsn()
            .saturating_sub(1)
            .max(self.recovery.checkpoint_lsn);
        let wal_bytes_before = fs::metadata(&self.path)?.len();
        let entries = state.len() as u64;
        let snapshot_bytes = write_snapshot(&self.snapshot_path, checkpoint_lsn, &state)?;

        let (archived_wal_bytes, archived_snapshot_bytes, pruned_archives) = if archive {
            let history = history_dir_for(&self.path);
            fs::create_dir_all(&history)?;
            let (snap_path, wal_path) = archive_paths(&history, checkpoint_lsn);
            let snap_bytes = archive_file(&self.snapshot_path, &snap_path)?;
            let wal_bytes = archive_segments(&wal.segment_paths()?, &wal_path)?;
            let pruned = if keep > 0 {
                Self::prune_history(&self.path, keep)?.removed.len()
            } else {
                0
            };
            (Some(wal_bytes), Some(snap_bytes), pruned)
        } else {
            (None, None, 0)
        };

        wal.reset(checkpoint_lsn.saturating_add(1).max(1))?;
        let wal_bytes_after = fs::metadata(&self.path)?.len();

        Ok(CheckpointInfo {
            checkpoint_lsn,
            entries,
            wal_bytes_before,
            wal_bytes_after,
            snapshot_bytes,
            archived_wal_bytes,
            archived_snapshot_bytes,
            pruned_archives,
        })
    }

    /// Reconstructs the committed state at `target_lsn` from archived
    /// checkpoints and the live WAL.
    ///
    /// The base is the newest archived checkpoint whose LSN is at or before
    /// `target_lsn`; if none exists, the live snapshot is used when it is old
    /// enough. Records after `target_lsn` are not applied, so a transaction
    /// whose `Commit` lands after the target is discarded.
    ///
    /// This does not open or modify the store. The caller can keep the live
    /// store open while inspecting history.
    pub fn recover_at(path: &Path, target_lsn: u64) -> Result<PitrView, TxnError> {
        let history = history_dir_for(path);
        let archives = list_archives(&history)?;
        let usable = archives
            .iter()
            .copied()
            .filter(|lsn| *lsn <= target_lsn)
            .max();

        // The base is a snapshot; every WAL record after its LSN is replayed
        // on top, up to `target_lsn`.
        let (checkpoint_lsn, state) = match usable {
            Some(lsn) => {
                let (snap_path, _) = archive_paths(&history, lsn);
                if !snap_path.is_file() {
                    return Err(TxnError::PitrIncompleteArchive {
                        checkpoint_lsn: lsn,
                        reason: "snapshot file is missing",
                    });
                }
                let snapshot =
                    read_snapshot(&snap_path)?.ok_or(TxnError::PitrIncompleteArchive {
                        checkpoint_lsn: lsn,
                        reason: "snapshot disappeared while reading",
                    })?;
                (snapshot.checkpoint_lsn, snapshot.entries)
            }
            None => {
                let live_snapshot_path = snapshot_path_for(path);
                let snapshot = read_snapshot(&live_snapshot_path)?;
                match snapshot {
                    Some(snapshot) if snapshot.checkpoint_lsn <= target_lsn => {
                        (snapshot.checkpoint_lsn, snapshot.entries)
                    }
                    Some(snapshot) => {
                        let oldest = archives.first().copied().unwrap_or(snapshot.checkpoint_lsn);
                        return Err(TxnError::PitrBeforeHistory {
                            target_lsn,
                            oldest_lsn: oldest,
                        });
                    }
                    None => (0, BTreeMap::new()),
                }
            }
        };

        // Archived WALs after the base, plus the live WAL. Archived WALs at or
        // before the base are covered by its snapshot.
        let mut wal_paths: Vec<PathBuf> = archives
            .iter()
            .copied()
            .filter(|lsn| *lsn > checkpoint_lsn)
            .map(|lsn| archive_paths(&history, lsn).1)
            .collect();
        wal_paths.push(path.to_path_buf());

        let mut records = Vec::new();
        let mut torn_tail_bytes = 0;
        for wal_path in &wal_paths {
            let Some(scan) = read_wal_scan(wal_path)? else {
                continue;
            };
            torn_tail_bytes = torn_tail_bytes.max(torn_bytes(&scan));
            records.extend(scan.records);
        }
        records.sort_by_key(|record| record.lsn);
        records.dedup_by_key(|record| record.lsn);

        let recovered =
            replay_records(&records, state, checkpoint_lsn, target_lsn, torn_tail_bytes)?;
        Ok(PitrView {
            target_lsn,
            checkpoint_lsn: recovered.info.checkpoint_lsn,
            applied_lsn: recovered.info.last_lsn,
            entries: recovered.state.into_iter().collect(),
            committed_txns: recovered.info.committed_txns,
            discarded_txns: recovered.info.discarded_txns,
        })
    }

    /// Reconstructs the committed state at the given wall-clock time.
    ///
    /// The time index is written at append time, so the result is the last
    /// commit that was indexed at or before `ts_ms`. If the index is missing
    /// or has no entry at or before the target, this returns
    /// [`TxnError::TimeIndexUnavailable`] instead of guessing.
    pub fn recover_at_time(path: &Path, ts_ms: u64) -> Result<PitrView, TxnError> {
        let lsn = tritdb_wal::lookup_time(path, ts_ms)?.ok_or(TxnError::TimeIndexUnavailable)?;
        Self::recover_at(path, lsn)
    }

    /// Like [`TxnStore::recover_at`], but holds the WAL writer lock while
    /// reading, so a concurrent checkpoint cannot swap the snapshot/archive
    /// base under the replay.
    pub fn recover_at_consistent(&self, target_lsn: u64) -> Result<PitrView, TxnError> {
        let _wal = self.wal.lock().map_err(|_| TxnError::Poisoned)?;
        Self::recover_at(&self.path, target_lsn)
    }

    /// Starts a background thread that checkpoints every `interval`.
    ///
    /// A checkpoint that races with active transactions returns
    /// [`TxnError::ActiveTxns`] and is retried on the next tick. Dropping the
    /// returned handle stops the thread and joins it.
    pub fn start_background_checkpoint(
        self: &Arc<Self>,
        interval: std::time::Duration,
    ) -> BackgroundCheckpoint {
        let store = Arc::clone(self);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(Ordering::SeqCst) {
                std::thread::sleep(interval);
                if stop_thread.load(Ordering::SeqCst) {
                    break;
                }
                let _ = store.checkpoint();
            }
        });
        BackgroundCheckpoint {
            stop,
            handle: Some(handle),
        }
    }

    /// Removes the oldest archived checkpoints, keeping the newest `keep`.
    ///
    /// Only complete archive pairs are considered. Removing an archive means
    /// [`TxnStore::recover_at`] can no longer target an LSN at or before the
    /// removed checkpoint; the live snapshot and newer archives are untouched.
    pub fn prune_history(path: &Path, keep: usize) -> Result<PruneInfo, TxnError> {
        let history = history_dir_for(path);
        let archives = list_archives(&history)?;
        if archives.len() <= keep {
            return Ok(PruneInfo {
                removed: Vec::new(),
                kept: archives,
            });
        }
        let split = archives.len() - keep;
        let (remove, keep_lsns) = archives.split_at(split);
        for lsn in remove {
            let (snap, wal) = archive_paths(&history, *lsn);
            // A missing `.snap` is not fatal: the `.wal` is the marker used by
            // `list_archives`, and a half archive is already unusable.
            let _ = fs::remove_file(&snap);
            fs::remove_file(&wal)?;
        }
        Ok(PruneInfo {
            removed: remove.to_vec(),
            kept: keep_lsns.to_vec(),
        })
    }

    fn wait_for_commit(&self, seq: u64) -> Result<(), TxnError> {
        let mut queue = self.commit_queue.lock().map_err(|_| TxnError::Poisoned)?;
        loop {
            if let Some(reason) = &queue.failure {
                return Err(TxnError::Writer {
                    reason: reason.clone(),
                });
            }
            if queue.synced_seq >= seq {
                return Ok(());
            }
            if queue.leader.is_none() {
                queue.leader = Some(seq);
                drop(queue);
                return self.run_commit_leader(seq);
            }
            queue = self
                .commit_ready
                .wait(queue)
                .map_err(|_| TxnError::Poisoned)?;
        }
    }

    fn run_commit_leader(&self, seq: u64) -> Result<(), TxnError> {
        // Let other committers append and enqueue before taking the WAL lock.
        for _ in 0..GROUP_COMMIT_YIELDS {
            std::thread::yield_now();
        }

        let mut wal = self.wal.lock().map_err(|_| TxnError::Poisoned)?;
        let mut queue = self.commit_queue.lock().map_err(|_| TxnError::Poisoned)?;

        if let Some(reason) = queue.failure.clone() {
            queue.leader = None;
            self.commit_ready.notify_all();
            return Err(TxnError::Writer { reason });
        }

        let works = std::mem::take(&mut queue.works);
        let max_seq = works.iter().map(|work| work.seq).max().unwrap_or(seq);

        if let Err(error) = wal.sync() {
            queue.failure = Some(error.to_string());
            queue.leader = None;
            self.commit_ready.notify_all();
            return Err(TxnError::Writer {
                reason: error.to_string(),
            });
        }
        self.syncs.fetch_add(1, Ordering::SeqCst);

        let mut state = match self.state.write() {
            Ok(state) => state,
            Err(_) => {
                queue.failure = Some("state lock poisoned".to_string());
                queue.leader = None;
                self.commit_ready.notify_all();
                return Err(TxnError::Poisoned);
            }
        };
        let mut works = works;
        works.sort_by_key(|work| work.commit_lsn);
        {
            let mut write_lsn = self.write_lsn.write().map_err(|_| TxnError::Poisoned)?;
            for work in &works {
                for op in &work.ops {
                    write_lsn.insert(op.key().to_vec(), work.commit_lsn);
                }
            }
        }
        for work in works {
            for op in work.ops {
                apply_op(&mut state, op);
            }
        }
        drop(state);

        queue.synced_seq = queue.synced_seq.max(max_seq);
        queue.leader = None;
        self.commit_ready.notify_all();
        drop(queue);
        drop(wal);
        Ok(())
    }
}

/// Handle for [`TxnStore::start_background_checkpoint`].
pub struct BackgroundCheckpoint {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for BackgroundCheckpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// A logical transaction. It does not hold the WAL mutex between calls.
pub struct Txn<'a> {
    store: &'a TxnStore,
    txn_id: u64,
    ops: Vec<Op>,
    active: bool,
    snapshot: Arc<BTreeMap<Vec<u8>, Vec<u8>>>,
    begin_lsn: u64,
}

impl<'a> Txn<'a> {
    /// Transaction id.
    pub const fn id(&self) -> u64 {
        self.txn_id
    }

    /// Reads a key inside this transaction's snapshot (read-your-writes).
    pub fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, TxnError> {
        for op in self.ops.iter().rev() {
            match op {
                Op::Put { key: op_key, value } if op_key == key => return Ok(Some(value.clone())),
                Op::Delete { key: op_key } if op_key == key => return Ok(None),
                _ => {}
            }
        }
        Ok(self.snapshot.get(key).cloned())
    }

    /// Returns the snapshot plus this transaction's buffered operations.
    pub fn iter(&self) -> Result<Vec<Entry>, TxnError> {
        let mut state = (*self.snapshot).clone();
        for op in &self.ops {
            apply_op(&mut state, op.clone());
        }
        Ok(state.into_iter().collect())
    }

    /// Logs a put and buffers it for commit.
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<u64, TxnError> {
        let payload = encode_put(key, value)?;
        let lsn = {
            let mut wal = self.store.wal.lock().map_err(|_| TxnError::Poisoned)?;
            wal.append(RecordKind::Put, self.txn_id, &payload, false)?
        };
        self.ops.push(Op::Put {
            key: key.to_vec(),
            value: value.to_vec(),
        });
        Ok(lsn)
    }

    /// Logs a delete and buffers it for commit.
    pub fn delete(&mut self, key: &[u8]) -> Result<u64, TxnError> {
        let payload = encode_key(key)?;
        let lsn = {
            let mut wal = self.store.wal.lock().map_err(|_| TxnError::Poisoned)?;
            wal.append(RecordKind::Delete, self.txn_id, &payload, false)?
        };
        self.ops.push(Op::Delete { key: key.to_vec() });
        Ok(lsn)
    }

    /// Appends `Commit`, joins the commit group, and returns after the group's
    /// fsync and state apply.
    pub fn commit(mut self) -> Result<CommitInfo, TxnError> {
        let result = self.commit_inner();
        self.finish();
        result
    }

    fn commit_inner(&mut self) -> Result<CommitInfo, TxnError> {
        let (commit_lsn, op_count, seq, is_leader) = {
            let mut wal = self.store.wal.lock().map_err(|_| TxnError::Poisoned)?;
            // First-committer-wins: any key written after this transaction
            // began has a newer commit LSN and aborts this commit.
            {
                let write_lsn = self
                    .store
                    .write_lsn
                    .read()
                    .map_err(|_| TxnError::Poisoned)?;
                for op in &self.ops {
                    if let Some(last_lsn) = write_lsn.get(op.key()) {
                        if *last_lsn >= self.begin_lsn {
                            return Err(TxnError::Conflict {
                                key: String::from_utf8_lossy(op.key()).into_owned(),
                                begin_lsn: self.begin_lsn,
                                last_lsn: *last_lsn,
                            });
                        }
                    }
                }
            }
            // Enqueue the work while still holding the WAL lock. A leader
            // cannot fsync and apply this commit before it enters the queue.
            let commit_lsn = wal.append(RecordKind::Commit, self.txn_id, &[], false)?;
            let op_count = self.ops.len();
            let ops = std::mem::take(&mut self.ops);

            let (seq, is_leader) = {
                let mut queue = self
                    .store
                    .commit_queue
                    .lock()
                    .map_err(|_| TxnError::Poisoned)?;
                if let Some(reason) = &queue.failure {
                    return Err(TxnError::Writer {
                        reason: reason.clone(),
                    });
                }
                let seq = queue.next_seq;
                queue.next_seq = queue.next_seq.saturating_add(1);
                queue.works.push(CommitWork {
                    seq,
                    commit_lsn,
                    ops,
                });
                let is_leader = queue.leader.is_none();
                if is_leader {
                    queue.leader = Some(seq);
                }
                (seq, is_leader)
            };

            drop(wal);
            (commit_lsn, op_count, seq, is_leader)
        };

        if is_leader {
            self.store.run_commit_leader(seq)?;
        } else {
            self.store.wait_for_commit(seq)?;
        }

        Ok(CommitInfo {
            txn_id: self.txn_id,
            commit_lsn,
            op_count,
        })
    }

    /// Appends `Abort` and drops buffered operations. Abort is not fsynced;
    /// losing it only means the transaction is discarded during recovery.
    pub fn abort(mut self) -> Result<u64, TxnError> {
        let abort_lsn = {
            let mut wal = self.store.wal.lock().map_err(|_| TxnError::Poisoned)?;
            wal.append(RecordKind::Abort, self.txn_id, &[], false)?
        };
        self.finish();
        Ok(abort_lsn)
    }

    fn finish(&mut self) {
        if self.active {
            self.active = false;
            self.store.active_txns.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl Drop for Txn<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Clone)]
enum Op {
    Put { key: Vec<u8>, value: Vec<u8> },
    Delete { key: Vec<u8> },
}

impl Op {
    fn key(&self) -> &[u8] {
        match self {
            Op::Put { key, .. } | Op::Delete { key } => key,
        }
    }
}

fn apply_op(state: &mut BTreeMap<Vec<u8>, Vec<u8>>, op: Op) {
    match op {
        Op::Put { key, value } => {
            state.insert(key, value);
        }
        Op::Delete { key } => {
            state.remove(&key);
        }
    }
}

struct Recovered {
    state: BTreeMap<Vec<u8>, Vec<u8>>,
    info: RecoveryInfo,
    max_txn_id: u64,
}

struct Snapshot {
    checkpoint_lsn: u64,
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

fn snapshot_path_for(wal_path: &Path) -> PathBuf {
    let mut name = wal_path.as_os_str().to_os_string();
    name.push(".snap");
    PathBuf::from(name)
}

fn snapshot_tmp_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

fn history_dir_for(wal_path: &Path) -> PathBuf {
    let mut name = wal_path.as_os_str().to_os_string();
    name.push(".history");
    PathBuf::from(name)
}

fn archive_paths(dir: &Path, lsn: u64) -> (PathBuf, PathBuf) {
    (
        dir.join(format!("{lsn}.snap")),
        dir.join(format!("{lsn}.wal")),
    )
}

/// Copies `src` to `dst` through a temporary file, so a crash never leaves a
/// half-written archive under the final name.
fn archive_file(src: &Path, dst: &Path) -> Result<u64, TxnError> {
    let tmp = {
        let mut name = dst.as_os_str().to_os_string();
        name.push(".tmp");
        PathBuf::from(name)
    };
    let bytes = fs::copy(src, &tmp)?;
    // `rename` does not overwrite on Windows; re-checkpointing the same LSN
    // should replace the old archive, not fail.
    let _ = fs::remove_file(dst);
    fs::rename(&tmp, dst)?;
    Ok(bytes)
}

/// Concatenates every WAL segment into one archive file.
///
/// Each source starts with its own `TRWS` header; `parse_wal` accepts a
/// sequence of segment headers, so the archive stays a single file.
fn archive_segments(sources: &[std::path::PathBuf], dst: &Path) -> Result<u64, TxnError> {
    let tmp = {
        let mut name = dst.as_os_str().to_os_string();
        name.push(".tmp");
        std::path::PathBuf::from(name)
    };
    let mut out = File::create(&tmp)?;
    let mut total = 0u64;
    for source in sources {
        if !source.is_file() {
            continue;
        }
        let bytes = fs::read(source)?;
        out.write_all(&bytes)?;
        total += bytes.len() as u64;
    }
    out.sync_all()?;
    drop(out);
    let _ = fs::remove_file(dst);
    fs::rename(&tmp, dst)?;
    Ok(total)
}

/// Lists archived checkpoint LSNs that have a `.wal` file.
fn list_archives(dir: &Path) -> Result<Vec<u64>, TxnError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut lsns = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("wal") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if let Ok(lsn) = stem.parse::<u64>() {
            lsns.push(lsn);
        }
    }
    lsns.sort_unstable();
    Ok(lsns)
}

fn read_snapshot(path: &Path) -> Result<Option<Snapshot>, TxnError> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let len = file.metadata()?.len();
    if len < SNAPSHOT_HEADER_SIZE as u64 {
        return Err(TxnError::BadSnapshot {
            reason: "truncated snapshot header",
        });
    }
    if len > SNAPSHOT_HEADER_SIZE as u64 + MAX_SNAPSHOT_PAYLOAD as u64 {
        return Err(TxnError::SnapshotTooLarge {
            len: (len - SNAPSHOT_HEADER_SIZE as u64) as usize,
            max: MAX_SNAPSHOT_PAYLOAD,
        });
    }

    let mut bytes = Vec::with_capacity(len as usize);
    file.read_to_end(&mut bytes)?;
    parse_snapshot(&bytes).map(Some)
}

fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, TxnError> {
    if bytes.len() < SNAPSHOT_HEADER_SIZE {
        return Err(TxnError::BadSnapshot {
            reason: "truncated snapshot header",
        });
    }
    if bytes[0..4] != SNAPSHOT_MAGIC {
        return Err(TxnError::BadSnapshot {
            reason: "bad snapshot magic",
        });
    }
    if bytes[4] != SNAPSHOT_VERSION {
        return Err(TxnError::BadSnapshot {
            reason: "bad snapshot version",
        });
    }
    if bytes[5..8] != [0u8; 3] {
        return Err(TxnError::BadSnapshot {
            reason: "snapshot reserved bytes are not zero",
        });
    }

    let checkpoint_lsn = u64::from_le_bytes([
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    ]);
    let entry_count = u64::from_le_bytes([
        bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
    ]);
    let payload_len = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as usize;
    if payload_len > MAX_SNAPSHOT_PAYLOAD {
        return Err(TxnError::SnapshotTooLarge {
            len: payload_len,
            max: MAX_SNAPSHOT_PAYLOAD,
        });
    }
    let expected_len =
        SNAPSHOT_HEADER_SIZE
            .checked_add(payload_len)
            .ok_or(TxnError::BadSnapshot {
                reason: "snapshot length overflow",
            })?;
    if bytes.len() != expected_len {
        return Err(TxnError::BadSnapshot {
            reason: "snapshot length does not match header",
        });
    }
    let expected_crc = u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]);
    let actual_crc = crc32_parts(&bytes[0..28], &bytes[SNAPSHOT_HEADER_SIZE..]);
    if actual_crc != expected_crc {
        return Err(TxnError::BadSnapshot {
            reason: "snapshot checksum mismatch",
        });
    }

    // Each entry costs at least 8 bytes (two zero-length blobs). Rejecting an
    // entry count that cannot fit prevents a tiny corrupt file from causing a
    // huge loop.
    if entry_count > (payload_len / 8) as u64 {
        return Err(TxnError::BadSnapshot {
            reason: "snapshot entry count exceeds payload",
        });
    }

    let payload = &bytes[SNAPSHOT_HEADER_SIZE..];
    let mut entries = BTreeMap::new();
    let mut offset = 0usize;
    for _ in 0..entry_count {
        let key = decode_snapshot_blob(payload, &mut offset, "snapshot key")?;
        let value = decode_snapshot_blob(payload, &mut offset, "snapshot value")?;
        if entries.insert(key, value).is_some() {
            return Err(TxnError::BadSnapshot {
                reason: "duplicate snapshot key",
            });
        }
    }
    if offset != payload.len() {
        return Err(TxnError::BadSnapshot {
            reason: "trailing snapshot bytes",
        });
    }

    Ok(Snapshot {
        checkpoint_lsn,
        entries,
    })
}

fn decode_snapshot_blob(
    payload: &[u8],
    offset: &mut usize,
    what: &'static str,
) -> Result<Vec<u8>, TxnError> {
    let length_end = offset
        .checked_add(4)
        .ok_or(TxnError::BadSnapshot { reason: what })?;
    if length_end > payload.len() {
        return Err(TxnError::BadSnapshot { reason: what });
    }
    let len = u32::from_le_bytes([
        payload[*offset],
        payload[*offset + 1],
        payload[*offset + 2],
        payload[*offset + 3],
    ]) as usize;
    let end = length_end
        .checked_add(len)
        .ok_or(TxnError::BadSnapshot { reason: what })?;
    if end > payload.len() {
        return Err(TxnError::BadSnapshot { reason: what });
    }
    let bytes = payload[length_end..end].to_vec();
    *offset = end;
    Ok(bytes)
}

fn write_snapshot(
    path: &Path,
    checkpoint_lsn: u64,
    state: &BTreeMap<Vec<u8>, Vec<u8>>,
) -> Result<u64, TxnError> {
    let mut payload = Vec::new();
    for (key, value) in state {
        let entry_len = 8usize
            .checked_add(key.len())
            .and_then(|len| len.checked_add(value.len()))
            .ok_or(TxnError::SnapshotTooLarge {
                len: usize::MAX,
                max: MAX_SNAPSHOT_PAYLOAD,
            })?;
        let new_len = payload
            .len()
            .checked_add(entry_len)
            .ok_or(TxnError::SnapshotTooLarge {
                len: usize::MAX,
                max: MAX_SNAPSHOT_PAYLOAD,
            })?;
        if new_len > MAX_SNAPSHOT_PAYLOAD {
            return Err(TxnError::SnapshotTooLarge {
                len: new_len,
                max: MAX_SNAPSHOT_PAYLOAD,
            });
        }
        let key_len = u32::try_from(key.len()).map_err(|_| TxnError::SnapshotTooLarge {
            len: key.len(),
            max: MAX_SNAPSHOT_PAYLOAD,
        })?;
        let value_len = u32::try_from(value.len()).map_err(|_| TxnError::SnapshotTooLarge {
            len: value.len(),
            max: MAX_SNAPSHOT_PAYLOAD,
        })?;
        payload.extend_from_slice(&key_len.to_le_bytes());
        payload.extend_from_slice(key);
        payload.extend_from_slice(&value_len.to_le_bytes());
        payload.extend_from_slice(value);
    }

    let payload_len = u32::try_from(payload.len()).map_err(|_| TxnError::SnapshotTooLarge {
        len: payload.len(),
        max: MAX_SNAPSHOT_PAYLOAD,
    })?;
    let mut header = [0u8; SNAPSHOT_HEADER_SIZE];
    header[0..4].copy_from_slice(&SNAPSHOT_MAGIC);
    header[4] = SNAPSHOT_VERSION;
    header[8..16].copy_from_slice(&checkpoint_lsn.to_le_bytes());
    header[16..24].copy_from_slice(&(state.len() as u64).to_le_bytes());
    header[24..28].copy_from_slice(&payload_len.to_le_bytes());
    let crc = crc32_parts(&header[0..28], &payload);
    header[28..32].copy_from_slice(&crc.to_le_bytes());

    let tmp = snapshot_tmp_path(path);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    file.write_all(&header)?;
    file.write_all(&payload)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&tmp, path)?;

    Ok((SNAPSHOT_HEADER_SIZE + payload.len()) as u64)
}

fn recover(
    scan: &WalScan,
    state: BTreeMap<Vec<u8>, Vec<u8>>,
    checkpoint_lsn: u64,
) -> Result<Recovered, TxnError> {
    let torn_tail_bytes = match scan.tail {
        tritdb_wal::TailState::Clean => 0,
        tritdb_wal::TailState::Torn { bytes, .. } => bytes,
    };
    replay_records(
        &scan.records,
        state,
        checkpoint_lsn,
        u64::MAX,
        torn_tail_bytes,
    )
}

/// Reads a WAL, retrying briefly when a concurrent checkpoint truncates it
/// mid-read. A missing file is not an error for archive/live discovery.
fn read_wal_scan(path: &Path) -> Result<Option<WalScan>, TxnError> {
    let mut last_error = None;
    for attempt in 0..3 {
        match tritdb_wal::WalReader::open(path) {
            Ok(scan) => return Ok(Some(scan)),
            Err(WalError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None)
            }
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }
    }
    Err(last_error.expect("at least one attempt").into())
}

fn torn_bytes(scan: &WalScan) -> u64 {
    match scan.tail {
        tritdb_wal::TailState::Clean => 0,
        tritdb_wal::TailState::Torn { bytes, .. } => bytes,
    }
}

fn replay_records(
    wal_records: &[tritdb_wal::Record],
    mut state: BTreeMap<Vec<u8>, Vec<u8>>,
    checkpoint_lsn: u64,
    max_lsn: u64,
    torn_tail_bytes: u64,
) -> Result<Recovered, TxnError> {
    let mut pending: BTreeMap<u64, Vec<Op>> = BTreeMap::new();
    let mut applied_lsn = checkpoint_lsn;
    let mut committed_txns = 0usize;
    let mut aborted_txns = 0usize;
    let mut max_txn_id = 0u64;
    let mut records = 0usize;
    let mut records_skipped = 0usize;

    for record in wal_records {
        max_txn_id = max_txn_id.max(record.txn_id);
        if record.lsn <= checkpoint_lsn || record.lsn > max_lsn {
            records_skipped += 1;
            continue;
        }
        records += 1;
        applied_lsn = applied_lsn.max(record.lsn);
        match record.kind {
            RecordKind::Begin => {
                if pending.contains_key(&record.txn_id) {
                    return Err(TxnError::DuplicateBegin {
                        txn_id: record.txn_id,
                    });
                }
                require_empty(record.kind, &record.payload)?;
                pending.insert(record.txn_id, Vec::new());
            }
            RecordKind::Put => {
                let op = decode_put(&record.payload)?;
                pending
                    .get_mut(&record.txn_id)
                    .ok_or(TxnError::UnknownTxn {
                        txn_id: record.txn_id,
                        kind: record.kind,
                    })?
                    .push(op);
            }
            RecordKind::Delete => {
                let op = decode_delete(&record.payload)?;
                pending
                    .get_mut(&record.txn_id)
                    .ok_or(TxnError::UnknownTxn {
                        txn_id: record.txn_id,
                        kind: record.kind,
                    })?
                    .push(op);
            }
            RecordKind::Commit => {
                require_empty(record.kind, &record.payload)?;
                let ops = pending
                    .remove(&record.txn_id)
                    .ok_or(TxnError::MissingBegin {
                        txn_id: record.txn_id,
                    })?;
                for op in ops {
                    apply_op(&mut state, op);
                }
                committed_txns += 1;
            }
            RecordKind::Abort => {
                require_empty(record.kind, &record.payload)?;
                pending
                    .remove(&record.txn_id)
                    .ok_or(TxnError::MissingBegin {
                        txn_id: record.txn_id,
                    })?;
                aborted_txns += 1;
            }
        }
    }

    Ok(Recovered {
        state,
        info: RecoveryInfo {
            records,
            records_skipped,
            checkpoint_lsn,
            committed_txns,
            aborted_txns,
            discarded_txns: pending.len(),
            torn_tail_bytes,
            last_lsn: applied_lsn,
        },
        max_txn_id,
    })
}

fn require_empty(kind: RecordKind, payload: &[u8]) -> Result<(), TxnError> {
    if payload.is_empty() {
        Ok(())
    } else {
        Err(TxnError::MalformedPayload {
            kind,
            reason: "payload must be empty",
        })
    }
}

fn encode_key(key: &[u8]) -> Result<Vec<u8>, TxnError> {
    let key_len = u32::try_from(key.len()).map_err(|_| TxnError::KeyTooLarge { len: key.len() })?;
    let mut payload = Vec::with_capacity(4 + key.len());
    payload.extend_from_slice(&key_len.to_le_bytes());
    payload.extend_from_slice(key);
    Ok(payload)
}

fn encode_put(key: &[u8], value: &[u8]) -> Result<Vec<u8>, TxnError> {
    let key_len = u32::try_from(key.len()).map_err(|_| TxnError::KeyTooLarge { len: key.len() })?;
    let value_len =
        u32::try_from(value.len()).map_err(|_| TxnError::ValueTooLarge { len: value.len() })?;
    let mut payload = Vec::with_capacity(4 + key.len() + 4 + value.len());
    payload.extend_from_slice(&key_len.to_le_bytes());
    payload.extend_from_slice(key);
    payload.extend_from_slice(&value_len.to_le_bytes());
    payload.extend_from_slice(value);
    Ok(payload)
}

fn decode_put(payload: &[u8]) -> Result<Op, TxnError> {
    let (key, offset) = decode_blob(payload, 0, RecordKind::Put, "put key")?;
    let (value, end) = decode_blob(payload, offset, RecordKind::Put, "put value")?;
    if end != payload.len() {
        return Err(TxnError::MalformedPayload {
            kind: RecordKind::Put,
            reason: "trailing bytes after put value",
        });
    }
    Ok(Op::Put { key, value })
}

fn decode_delete(payload: &[u8]) -> Result<Op, TxnError> {
    let (key, end) = decode_blob(payload, 0, RecordKind::Delete, "delete key")?;
    if end != payload.len() {
        return Err(TxnError::MalformedPayload {
            kind: RecordKind::Delete,
            reason: "trailing bytes after delete key",
        });
    }
    Ok(Op::Delete { key })
}

fn decode_blob(
    payload: &[u8],
    offset: usize,
    kind: RecordKind,
    what: &'static str,
) -> Result<(Vec<u8>, usize), TxnError> {
    let length_end = offset
        .checked_add(4)
        .ok_or(TxnError::MalformedPayload { kind, reason: what })?;
    if length_end > payload.len() {
        return Err(TxnError::MalformedPayload { kind, reason: what });
    }
    let len = u32::from_le_bytes([
        payload[offset],
        payload[offset + 1],
        payload[offset + 2],
        payload[offset + 3],
    ]) as usize;
    let end = length_end
        .checked_add(len)
        .ok_or(TxnError::MalformedPayload { kind, reason: what })?;
    if end > payload.len() {
        return Err(TxnError::MalformedPayload { kind, reason: what });
    }
    Ok((payload[length_end..end].to_vec(), end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::Arc;
    use std::thread;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "tritdb-txn-test-{}-{}",
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
    fn commit_and_recover() {
        let dir = TempDir::new();
        let path = dir.wal();
        {
            let store = TxnStore::open(&path).expect("open");
            let mut txn = store.begin().expect("begin");
            txn.put(b"a", b"1").expect("put a");
            txn.put(b"b", b"2").expect("put b");
            let info = txn.commit().expect("commit");
            assert_eq!(info.op_count, 2);

            let mut txn = store.begin().expect("begin");
            txn.delete(b"a").expect("delete a");
            txn.commit().expect("commit delete");
            assert_eq!(store.get(b"a").expect("get"), None);
            assert_eq!(store.get(b"b").expect("get"), Some(b"2".to_vec()));
        }

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.get(b"a").expect("get"), None);
        assert_eq!(store.get(b"b").expect("get"), Some(b"2".to_vec()));
        assert_eq!(store.len().expect("len"), 1);
        assert_eq!(store.recovery().records, 7);
        assert_eq!(store.recovery().committed_txns, 2);
        assert_eq!(store.recovery().aborted_txns, 0);
        assert_eq!(store.recovery().discarded_txns, 0);
        assert_eq!(store.recovery().torn_tail_bytes, 0);
    }

    #[test]
    fn checkpoint_writes_snapshot_and_resets_wal() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = TxnStore::open(&path).expect("open");
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"1").expect("put a");
        txn.put(b"b", b"2").expect("put b");
        txn.commit().expect("commit");

        let info = store.checkpoint().expect("checkpoint");
        assert_eq!(info.entries, 2);
        assert_eq!(info.wal_bytes_after, 32);
        assert_eq!(info.snapshot_bytes, 52);
        assert!(info.wal_bytes_before > info.wal_bytes_after);
        assert!(store.snapshot_path().exists());
        let checkpoint_lsn = info.checkpoint_lsn;
        drop(store);

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.get(b"a").expect("get a"), Some(b"1".to_vec()));
        assert_eq!(store.get(b"b").expect("get b"), Some(b"2".to_vec()));
        assert_eq!(store.recovery().records, 0);
        assert_eq!(store.recovery().records_skipped, 0);
        assert_eq!(store.recovery().checkpoint_lsn, checkpoint_lsn);
        assert_eq!(store.recovery().last_lsn, checkpoint_lsn);
    }

    #[test]
    fn checkpoint_skips_stale_wal_records() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = TxnStore::open(&path).expect("open");
        let mut txn = store.begin().expect("begin");
        txn.put(b"kept", b"value").expect("put");
        txn.commit().expect("commit");
        let old_wal = fs::read(&path).expect("read old wal");
        let info = store.checkpoint().expect("checkpoint");
        let checkpoint_lsn = info.checkpoint_lsn;
        drop(store);

        // Simulate a crash after the snapshot is durable but before the WAL
        // reset: put the old WAL bytes back next to the new snapshot.
        fs::write(&path, old_wal).expect("restore old wal");

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.get(b"kept").expect("get"), Some(b"value".to_vec()));
        assert_eq!(store.recovery().records, 0);
        assert_eq!(store.recovery().records_skipped, 3);
        assert_eq!(store.recovery().checkpoint_lsn, checkpoint_lsn);
        assert_eq!(store.recovery().last_lsn, checkpoint_lsn);
    }

    #[test]
    fn checkpoint_replaces_existing_snapshot() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = TxnStore::open(&path).expect("open");
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"1").expect("put a");
        txn.commit().expect("commit");
        let first = store.checkpoint().expect("first checkpoint");

        let mut txn = store.begin().expect("begin");
        txn.put(b"b", b"2").expect("put b");
        txn.commit().expect("commit");
        let second = store.checkpoint().expect("second checkpoint");
        assert!(second.checkpoint_lsn > first.checkpoint_lsn);
        assert_eq!(second.entries, 2);
        drop(store);

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.get(b"a").expect("get a"), Some(b"1".to_vec()));
        assert_eq!(store.get(b"b").expect("get b"), Some(b"2".to_vec()));
        assert_eq!(store.recovery().records, 0);
        assert_eq!(store.recovery().checkpoint_lsn, second.checkpoint_lsn);
    }

    #[test]
    fn snapshot_corruption_is_rejected() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = TxnStore::open(&path).expect("open");
        let mut txn = store.begin().expect("begin");
        txn.put(b"a", b"1").expect("put");
        txn.commit().expect("commit");
        store.checkpoint().expect("checkpoint");
        let snapshot_path = store.snapshot_path().to_path_buf();
        drop(store);

        let mut bytes = fs::read(&snapshot_path).expect("read snapshot");
        bytes[28] ^= 0x01; // corrupt the stored CRC
        fs::write(&snapshot_path, bytes).expect("write corrupt snapshot");

        let error = TxnStore::open(&path).err().expect("open must fail");
        assert!(matches!(error, TxnError::BadSnapshot { .. }));
    }

    #[test]
    fn uncommitted_txn_is_discarded() {
        let dir = TempDir::new();
        let path = dir.wal();
        {
            let store = TxnStore::open(&path).expect("open");
            let mut txn = store.begin().expect("begin");
            txn.put(b"ghost", b"value").expect("put");
            drop(txn);
            assert_eq!(store.get(b"ghost").expect("get in memory"), None);
        }

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.get(b"ghost").expect("get"), None);
        assert_eq!(store.recovery().committed_txns, 0);
        assert_eq!(store.recovery().discarded_txns, 1);
    }

    #[test]
    fn abort_discards_ops() {
        let dir = TempDir::new();
        let path = dir.wal();
        {
            let store = TxnStore::open(&path).expect("open");
            let mut txn = store.begin().expect("begin");
            txn.put(b"aborted", b"value").expect("put");
            let lsn = txn.abort().expect("abort");
            assert!(lsn > 0);
        }

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.get(b"aborted").expect("get"), None);
        assert_eq!(store.recovery().aborted_txns, 1);
        assert_eq!(store.recovery().discarded_txns, 0);
    }

    #[test]
    fn multiple_readers_see_committed_values() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = Arc::new(TxnStore::open(&path).expect("open"));
        for index in 0..8u64 {
            let mut txn = store.begin().expect("begin");
            txn.put(
                format!("k{index}").as_bytes(),
                format!("v{index}").as_bytes(),
            )
            .expect("put");
            txn.commit().expect("commit");
        }

        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                for _ in 0..1_000 {
                    assert_eq!(
                        store.get(b"k7").expect("get"),
                        Some(b"v7".to_vec()),
                        "reader saw a stale or missing value"
                    );
                }
            }));
        }
        for handle in handles {
            handle.join().expect("reader thread");
        }
    }

    #[test]
    fn readers_never_see_a_partial_commit() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = Arc::new(TxnStore::open(&path).expect("open"));
        let stop = Arc::new(AtomicBool::new(false));

        let reader = {
            let store = Arc::clone(&store);
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    // `iter` takes one read lock and clones the whole map, so
                    // it observes a single committed state. Four separate
                    // `get` calls would not be a cross-key snapshot.
                    let entries = store.iter().expect("iter");
                    let present = entries
                        .iter()
                        .filter(|(key, _)| key.starts_with(b"g"))
                        .count();
                    assert!(
                        present == 0 || present == 4,
                        "reader saw {present}/4 keys of one commit"
                    );
                }
            })
        };

        for txn in 0..20u64 {
            let mut transaction = store.begin().expect("begin");
            for index in 0..4u64 {
                transaction
                    .put(
                        format!("g{index}").as_bytes(),
                        format!("v{txn}-{index}").as_bytes(),
                    )
                    .expect("put");
            }
            transaction.commit().expect("commit");
        }

        stop.store(true, Ordering::Relaxed);
        reader.join().expect("reader thread");
    }

    #[test]
    fn torn_tail_is_truncated_on_open() {
        let dir = TempDir::new();
        let path = dir.wal();
        {
            let store = TxnStore::open(&path).expect("open");
            let mut txn = store.begin().expect("begin");
            txn.put(b"kept", b"value").expect("put");
            txn.commit().expect("commit");
        }

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open for append");
        file.write_all(b"TRW").expect("write torn bytes");
        drop(file);

        let store = TxnStore::open(&path).expect("recover torn");
        assert_eq!(store.get(b"kept").expect("get"), Some(b"value".to_vec()));
        assert_eq!(store.recovery().torn_tail_bytes, 3);
        drop(store);

        let store = TxnStore::open(&path).expect("recover clean");
        assert_eq!(store.recovery().torn_tail_bytes, 0);
        assert_eq!(store.get(b"kept").expect("get"), Some(b"value".to_vec()));
    }

    #[test]
    fn store_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TxnStore>();
    }

    #[test]
    fn checkpoint_rejects_active_transactions() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = TxnStore::open(&path).expect("open");
        let _txn = store.begin().expect("begin");
        let error = store.checkpoint().expect_err("checkpoint must fail");
        assert!(matches!(error, TxnError::ActiveTxns { count: 1 }));
    }

    #[test]
    fn concurrent_commits_share_fsyncs_and_recover() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = Arc::new(TxnStore::open(&path).expect("open"));
        let barrier = Arc::new(std::sync::Barrier::new(16));
        let mut handles = Vec::new();

        for index in 0..16u64 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                let mut txn = store.begin().expect("begin");
                txn.put(
                    format!("k{index}").as_bytes(),
                    format!("v{index}").as_bytes(),
                )
                .expect("put");
                txn.commit().expect("commit");
            }));
        }
        for handle in handles {
            handle.join().expect("writer thread");
        }

        assert_eq!(store.len().expect("len"), 16);
        assert_eq!(store.active_txns(), 0);
        let syncs = store.sync_count();
        assert!(syncs >= 1);
        assert!(
            syncs < 16,
            "16 concurrent commits used {syncs} fsyncs; no batching happened"
        );
        drop(store);

        let store = TxnStore::open(&path).expect("reopen");
        assert_eq!(store.len().expect("len"), 16);
        assert_eq!(store.recovery().committed_txns, 16);
        assert_eq!(store.recovery().discarded_txns, 0);
    }

    #[test]
    fn tiny_segment_limit_still_recovers_every_record() {
        let dir = TempDir::new();
        let path = dir.wal();
        // The writer clamps the limit to MIN_SEGMENT_LIMIT, so a 1-byte
        // request must not spin forever or drop records at a segment boundary.
        let store = TxnStore::open_with_wal_limit(&path, 1).expect("open");
        for index in 0..50u64 {
            let mut txn = store.begin().expect("begin");
            txn.put(format!("k{index}").as_bytes(), b"v").expect("put");
            txn.commit().expect("commit");
        }
        drop(store);

        let reopened = TxnStore::open_with_wal_limit(&path, 1).expect("reopen");
        for index in 0..50u64 {
            let key = format!("k{index}");
            assert_eq!(
                reopened.get(key.as_bytes()).expect("get"),
                Some(b"v".to_vec()),
                "record {key} lost across rotated segments"
            );
        }
    }

    #[test]
    fn pitr_recovers_across_wal_segments() {
        let dir = TempDir::new();
        let path = dir.wal();
        // A tiny segment limit forces several WAL segments before the archive.
        let store = TxnStore::open_with_wal_limit(&path, 1024).expect("open");
        let mut last_lsn = 0;
        for index in 0..20u64 {
            let mut txn = store.begin().expect("begin");
            txn.put(format!("k{index}").as_bytes(), b"v").expect("put");
            last_lsn = txn.commit().expect("commit").commit_lsn;
        }
        store.checkpoint_archive_keep(8).expect("archive");
        drop(store);

        let view = TxnStore::recover_at(&path, last_lsn).expect("pitr across segments");
        assert_eq!(view.entries.len(), 20);
        for index in 0..20u64 {
            let key = format!("k{index}");
            assert!(
                view.entries
                    .iter()
                    .any(|(entry, _)| entry == key.as_bytes()),
                "missing {key} after cross-segment PITR"
            );
        }

        // Pruning history down to one archive must keep the latest target
        // recoverable.
        let store = TxnStore::open(&path).expect("reopen");
        store.checkpoint_archive_keep(1).expect("archive keep 1");
        drop(store);
        let view = TxnStore::recover_at(&path, last_lsn).expect("pitr after prune");
        assert_eq!(view.entries.len(), 20);
    }

    #[test]
    fn concurrent_archived_checkpoints_keep_recovery_intact() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = Arc::new(TxnStore::open(&path).expect("open"));
        let mut txn = store.begin().expect("begin");
        txn.put(b"k", b"v").expect("put");
        txn.commit().expect("commit");

        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                for _ in 0..5 {
                    // Serialized by the WAL lock; archives must stay complete.
                    let _ = store.checkpoint_archive_keep(4);
                }
            }));
        }
        for handle in handles {
            handle.join().expect("join");
        }
        // `TxnStore::open` takes `<wal>.lock`; release it before reopening.
        drop(store);

        let reopened = TxnStore::open(&path).expect("reopen");
        assert_eq!(reopened.get(b"k").expect("get"), Some(b"v".to_vec()));
    }

    #[test]
    fn checkpoint_races_begin_without_losing_commits() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = Arc::new(TxnStore::open(&path).expect("open"));
        let mut handles = Vec::new();
        for worker in 0..4u64 {
            let store = Arc::clone(&store);
            handles.push(thread::spawn(move || {
                for index in 0..50u64 {
                    let key = format!("w{worker}-{index}");
                    let mut txn = store.begin().expect("begin");
                    txn.put(key.as_bytes(), b"v").expect("put");
                    txn.commit().expect("commit");
                }
            }));
        }
        let mut checkpoints = 0;
        let mut attempts = 0;
        while checkpoints < 20 {
            attempts += 1;
            assert!(attempts < 100_000, "checkpoint never made progress");
            match store.checkpoint() {
                Ok(_) => checkpoints += 1,
                Err(TxnError::ActiveTxns { .. }) => std::thread::yield_now(),
                Err(error) => panic!("checkpoint failed: {error}"),
            }
        }
        for handle in handles {
            handle.join().expect("join");
        }
        for worker in 0..4u64 {
            for index in 0..50u64 {
                let key = format!("w{worker}-{index}");
                assert_eq!(
                    store.get(key.as_bytes()).expect("get"),
                    Some(b"v".to_vec()),
                    "committed key {key} was lost"
                );
            }
        }
        drop(store);
        let reopened = TxnStore::open(&path).expect("reopen");
        for worker in 0..4u64 {
            for index in 0..50u64 {
                let key = format!("w{worker}-{index}");
                assert_eq!(
                    reopened.get(key.as_bytes()).expect("get"),
                    Some(b"v".to_vec()),
                    "committed key {key} did not survive reopen"
                );
            }
        }
    }

    #[test]
    fn commit_group_applies_disjoint_keys() {
        let dir = TempDir::new();
        let path = dir.wal();
        let store = TxnStore::open(&path).expect("open");

        let mut first = store.begin().expect("begin first");
        first.put(b"a", b"1").expect("put first");
        let mut second = store.begin().expect("begin second");
        second.put(b"b", b"2").expect("put second");
        first.commit().expect("commit first");
        second.commit().expect("commit second");

        assert_eq!(store.get(b"a").expect("get a"), Some(b"1".to_vec()));
        assert_eq!(store.get(b"b").expect("get b"), Some(b"2".to_vec()));
    }
}
