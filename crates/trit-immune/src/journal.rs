//! 追加式日志：DeltaDB 的落盘形态——**日志即数据库**（v2：每条 Delta 带 HLC 时间戳）。
//!
//! - [`Journal::create`]：写头部 + 盖戳的 `ROOT`，之后每条变更追加一行
//!   `<物理毫秒>:<计数>:<写者>\t<OP>\t...`；
//! - [`Journal::open`]：优先用快照（`<journal>.snapshot`，只是重放缓存）恢复，
//!   再只重放尾部 Delta；快照损坏/不匹配自动回退全量重放；
//!   重放时**观察全部 HLC 戳**（非递增的日志直接拒绝），保证后续追加因果更晚；
//! - [`Journal::snapshot`]：把当前全量状态写成快照（临时文件 + 原子改名）；
//! - 写盘失败进入 poison 状态，拒绝继续追加（内存/磁盘不静默分叉），重新 `open` 即恢复一致。

use crate::dag::{ImmuneDag, NodeId, NodeKind};
use crate::delta::{self, ApplyError, Delta, Stamped};
use crate::hlc::{Clock, Hlc};
use crate::snapshot::{self, SnapshotMeta};
use crate::trit::Trit;
use crate::verdict::ExploitEvidence;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 日志文件头（第一行）。
pub const HEADER: &str = "trit-immune-journal v2";

/// 打开日志的错误。
#[derive(Debug)]
pub enum OpenError {
    Io(io::Error),
    Header(String),
    Parse { line: usize, message: String },
    Apply { line: usize, source: ApplyError },
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO 错误：{e}"),
            Self::Header(h) => write!(f, "日志头部不合法：{h:?}"),
            Self::Parse { line, message } => write!(f, "第 {line} 行解析失败：{message}"),
            Self::Apply { line, source } => write!(f, "第 {line} 行 apply 失败：{source}"),
        }
    }
}

impl std::error::Error for OpenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Apply { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// 追加写错误。
#[derive(Debug)]
pub enum JournalError {
    Io(io::Error),
    Apply(ApplyError),
}

impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO 错误：{e}"),
            Self::Apply(e) => write!(f, "apply 失败：{e}"),
        }
    }
}

impl std::error::Error for JournalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Apply(e) => Some(e),
        }
    }
}

impl From<io::Error> for JournalError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<ApplyError> for JournalError {
    fn from(e: ApplyError) -> Self {
        Self::Apply(e)
    }
}

/// 追加式日志句柄：内存 DAG + 只增文件（+ 可选快照缓存 + HLC 时钟）。
#[derive(Debug)]
pub struct Journal {
    dag: ImmuneDag,
    file: File,
    path: PathBuf,
    poisoned: bool,
    clock: Clock,
    /// 已应用（日志已覆盖）的 Delta 条数。
    applied: usize,
    /// 日志中已被应用覆盖的字节前缀长度。
    covered_bytes: u64,
    /// 本次 open 是否从快照恢复。
    from_snapshot: bool,
    /// 本次 open 在快照之后重放的尾部 Delta 条数。
    tail_replayed: usize,
    /// 备份模式：Auto = adopt/migrate 时自动备份；Manual = 调用方自己决定。
    pub backup_mode: BackupMode,
}

/// 备份模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackupMode {
    /// 自动：adopt/migrate 时自动创建 `.backup` 文件（默认，安全优先）。
    Auto,
    /// 手动：调用方自己调用 `backup()` 决定何时备份。
    Manual,
}

impl Journal {
    /// 新建日志文件（已存在则报错）：写入头部 + 盖戳的 `ROOT`，写者 id = 0。
    pub fn create(path: impl AsRef<Path>, root_title: &str) -> Result<Self, JournalError> {
        Self::create_as(path, root_title, 0)
    }

    /// 指定写者 id 新建（多副本/多 Agent 场景下用于 HLC 破平）。
    pub fn create_as(
        path: impl AsRef<Path>,
        root_title: &str,
        writer: u32,
    ) -> Result<Self, JournalError> {
        let mut clock = Clock::new(writer);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.as_ref())?;
        let ts = clock.tick();
        let mut head = String::new();
        head.push_str(HEADER);
        head.push('\n');
        head.push_str(&format!(
            "{}\t{}",
            ts,
            delta::encode(&Delta::Root {
                title: root_title.to_string(),
            })
        ));
        head.push('\n');
        file.write_all(head.as_bytes())?;
        file.flush()?;
        let mut dag = ImmuneDag::empty();
        dag.apply(&Delta::Root {
            title: root_title.to_string(),
        })?;
        Ok(Self {
            dag,
            file,
            path: path.as_ref().to_path_buf(),
            poisoned: false,
            clock,
            applied: 1,
            covered_bytes: head.len() as u64,
            from_snapshot: false,
            tail_replayed: 0,
            backup_mode: BackupMode::Auto,
        })
    }

    /// 打开日志（写者 id = 0）。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, OpenError> {
        Self::open_as(path, 0)
    }

    /// 指定写者 id 打开：优先从快照恢复（快照只是缓存，损坏/不匹配自动回退全量重放），
    /// 再只重放快照覆盖点之后的尾部 Delta；重放时观察全部 HLC 戳，
    /// 保证后续追加因果更晚。
    pub fn open_as(path: impl AsRef<Path>, writer: u32) -> Result<Self, OpenError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path).map_err(OpenError::Io)?;
        if text.is_empty() {
            return Err(OpenError::Header("空文件".into()));
        }
        let raw = text.as_bytes();

        let mut dag = ImmuneDag::empty();
        let mut applied = 0usize;
        let mut covered = 0usize;
        let mut from_snapshot = false;
        if let Ok(snap_text) = std::fs::read_to_string(snapshot::sidecar_path(path)) {
            if let Ok((snap_dag, meta)) = snapshot::parse(&snap_text) {
                let plen = meta.prefix_len as usize;
                if plen <= raw.len() {
                    let prefix = &raw[..plen];
                    let newlines = prefix.iter().filter(|&&b| b == b'\n').count();
                    if snapshot::fnv1a_bytes(prefix) == meta.prefix_fnv
                        && newlines == meta.deltas_covered + 1
                    {
                        dag = snap_dag;
                        applied = meta.deltas_covered;
                        covered = plen;
                        from_snapshot = true;
                    }
                }
            }
        }

        // 剩余文本：快照路径没有头部（前缀已含）；全量路径先校验头部
        let (rest, line_offset) = if from_snapshot {
            (&text[covered..], applied + 1)
        } else {
            let (header, rest) = match text.split_once('\n') {
                Some((h, r)) => (h, r),
                None => (text.as_str(), ""),
            };
            if header.trim_end_matches('\r') != HEADER {
                return Err(OpenError::Header(header.to_string()));
            }
            (rest, 1usize)
        };

        let mut clock = Clock::new(writer);
        let mut prev_stamp: Option<Hlc> = None;
        if from_snapshot {
            // 从覆盖前缀的最后一行恢复监视点（缓存语义：解析失败就跳过）
            if let Some(last_line) = text[..covered].lines().rev().find(|l| !l.trim().is_empty()) {
                if let Ok((ts, _)) = parse_stamped_line(last_line) {
                    prev_stamp = Some(ts);
                    clock.observe(&ts);
                }
            }
        }

        let mut tail = 0usize;
        for (i, line) in rest.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let (ts, d) = parse_stamped_line(line)
                .map_err(|message| OpenError::Parse {
                    line: line_offset + i + 1,
                    message,
                })?;
            if let Some(prev) = prev_stamp {
                if ts <= prev {
                    return Err(OpenError::Parse {
                        line: line_offset + i + 1,
                        message: format!("时间戳非递增：{ts} ≤ {prev}"),
                    });
                }
            }
            prev_stamp = Some(ts);
            clock.observe(&ts);
            dag.apply(&d).map_err(|source| OpenError::Apply {
                line: line_offset + i + 1,
                source,
            })?;
            applied += 1;
            tail += 1;
        }

        let file = OpenOptions::new()
            .append(true)
            .open(path)
            .map_err(OpenError::Io)?;
        Ok(Self {
            dag,
            file,
            path: path.to_path_buf(),
            poisoned: false,
            clock,
            applied,
            covered_bytes: text.len() as u64,
            from_snapshot,
            tail_replayed: tail,
            backup_mode: BackupMode::Auto,
        })
    }

    pub fn dag(&self) -> &ImmuneDag {
        &self.dag
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 本次 open 是否从快照恢复。
    pub fn recovered_from_snapshot(&self) -> bool {
        self.from_snapshot
    }

    /// 本次 open 在快照之后重放的尾部 Delta 条数。
    pub fn tail_replayed(&self) -> usize {
        self.tail_replayed
    }

    /// 已应用（日志已覆盖）的 Delta 条数。
    pub fn applied_deltas(&self) -> usize {
        self.applied
    }

    /// 本 journal 的 HLC 时钟（重放时已观察全部戳）。
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    /// 同步场景：观察远端戳，保证本副本后续追加在全序上因果更晚。
    pub fn observe_remote(&mut self, ts: &Hlc) {
        self.clock.observe(ts);
    }

    /// 采纳合并后的规范日志：原子重写日志文件并重建内存状态（跨副本同步用）。
    /// BackupMode::Auto 时自动备份旧日志到 `.backup`。
    pub fn adopt(&mut self, canonical: Vec<Stamped>) -> io::Result<()> {
        if self.backup_mode == BackupMode::Auto {
            let _ = self.backup();
        }
        let mut head = String::new();
        head.push_str(HEADER);
        head.push('\n');
        for s in &canonical {
            head.push_str(&format!("{}\t{}", s.ts, delta::encode(&s.delta)));
            head.push('\n');
        }
        let tmp = self.path.with_extension("journal.tmp");
        std::fs::write(&tmp, head.as_bytes())?;
        std::fs::rename(&tmp, &self.path)?;

        let mut dag = ImmuneDag::empty();
        let mut clock = Clock::new(self.clock.writer());
        for s in &canonical {
            clock.observe(&s.ts);
            dag.apply(&s.delta)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        }
        self.file = OpenOptions::new().append(true).open(&self.path)?;
        self.dag = dag;
        self.clock = clock;
        self.applied = canonical.len();
        self.covered_bytes = head.len() as u64;
        self.poisoned = false;
        let _ = std::fs::remove_file(snapshot::sidecar_path(&self.path));
        Ok(())
    }

    /// 把当前全量状态写成快照（`<journal>.snapshot`，临时文件 + 原子改名）。
    /// 快照只是重放缓存：损坏/不匹配时 `open` 自动回退全量重放。
    pub fn snapshot(&self) -> io::Result<PathBuf> {
        let raw = std::fs::read(&self.path)?;
        let plen = (self.covered_bytes as usize).min(raw.len());
        let meta = SnapshotMeta {
            deltas_covered: self.applied,
            prefix_len: plen as u64,
            prefix_fnv: snapshot::fnv1a_bytes(&raw[..plen]),
        };
        let snap = snapshot::sidecar_path(&self.path);
        let tmp = snap.with_extension("snapshot.tmp");
        std::fs::write(&tmp, snapshot::encode(&self.dag, &meta))?;
        std::fs::rename(&tmp, &snap)?;
        Ok(snap)
    }

    /// 备份当前日志文件到 `<path>.backup`（原子复制）。
    pub fn backup(&self) -> io::Result<PathBuf> {
        let backup = self.path.with_extension("journal.backup");
        std::fs::copy(&self.path, &backup)?;
        Ok(backup)
    }

    /// 追加一条 Delta：先内存 apply（校验），再落盘。
    /// 落盘失败后进入 poison 状态，拒绝继续追加，直到重新 [`Journal::open`]。
    pub fn append_delta(&mut self, d: &Delta) -> Result<(), JournalError> {
        if self.poisoned {
            return Err(JournalError::Io(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "journal 已进入 poison 状态（上次写盘失败），请重新 open 恢复一致",
            )));
        }
        self.dag.apply(d)?;
        let ts = self.clock.tick();
        let mut line = format!("{}\t{}", ts, delta::encode(d));
        line.push('\n');
        if let Err(e) = self
            .file
            .write_all(line.as_bytes())
            .and_then(|()| self.file.flush())
        {
            self.poisoned = true;
            return Err(JournalError::Io(e));
        }
        self.applied += 1;
        self.covered_bytes += line.len() as u64;
        Ok(())
    }

    pub fn spawn(
        &mut self,
        parent: NodeId,
        kind: NodeKind,
        title: &str,
    ) -> Result<NodeId, JournalError> {
        self.append_delta(&Delta::Spawn {
            parent,
            kind,
            title: title.to_string(),
        })?;
        Ok(NodeId((self.dag.nodes().len() - 1) as u32))
    }

    pub fn record(
        &mut self,
        id: NodeId,
        verdict: Trit,
        evidence: Option<ExploitEvidence>,
        note: &str,
    ) -> Result<(), JournalError> {
        self.append_delta(&Delta::Record {
            id,
            verdict,
            evidence,
            note: note.to_string(),
        })
    }

    pub fn quarantine(&mut self, id: NodeId, note: &str) -> Result<(), JournalError> {
        self.append_delta(&Delta::Quarantine {
            id,
            note: note.to_string(),
        })
    }

    pub fn human_review(
        &mut self,
        id: NodeId,
        verdict: Trit,
        note: &str,
    ) -> Result<(), JournalError> {
        self.append_delta(&Delta::HumanReview {
            id,
            verdict,
            note: note.to_string(),
        })
    }

    pub fn cross_link(
        &mut self,
        from: NodeId,
        to: NodeId,
        pass: Trit,
        note: &str,
    ) -> Result<(), JournalError> {
        self.append_delta(&Delta::CrossLink {
            from,
            to,
            pass,
            note: note.to_string(),
        })
    }

    /// 崩溃持久化：flush 只保证进程内一致；`sync_all` 才真正落盘。
    pub fn sync(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// 消费日志，取出内存 DAG（先 flush）。
    pub fn into_dag(mut self) -> io::Result<ImmuneDag> {
        self.file.flush()?;
        Ok(self.dag)
    }
}

/// 解析 v2 日志行：`<物理>:<计数>:<写者>\t<OP>\t...`（工具与同步层共用）。
pub fn parse_stamped_line(line: &str) -> Result<(Hlc, Delta), String> {
    let (ts_field, rest) = line.split_once('\t').ok_or("缺少时间戳字段")?;
    let ts = ts_field.parse::<Hlc>()?;
    let d = delta::parse_line(rest)?;
    Ok((ts, d))
}
