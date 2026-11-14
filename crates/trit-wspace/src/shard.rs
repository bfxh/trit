//! 审计日志分片：每个分片一个 header + 分片内 oracle 表。
//!
//! 长日志按条数切成多个分片文件；第一条是 header（JSON），其后是原样的 v1 记录行。
//! header 声明本分片的序号/起止、父分片链头、本分片链头，以及**分片内 oracle 表**：
//! 记录行不再内嵌验证集，只写 `oracle_index` 指向表项（同一验证集在一个分片内只存一次）。
//! 链字段保持不变：`chain_of` 用解析后的完整验证集重算，跨分片仍是同一条哈希链。
//!
//! 校验分两层：加载时验表（索引合法、无重复、无未引用项、表摘要匹配），
//! [`ShardedLog::verify_chain`] 再验跨分片的 `seq` / `prev` / `head` 与逐条 `chain`。

use serde_json::{Value, json};

use crate::audit::{
    AuditLog, GENESIS, LogEntry, ReplayReport, ReplayState, canonical, chain_of, fnv, hex,
    parse_entry, replay_entries,
};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::wire;
/// 分片 header 的版本。
pub const SHARD_VERSION: u64 = 2;

/// 分片 header 的 `kind` 值。
pub const SHARD_KIND: &str = "wspace-shard";

/// 分片头。
#[derive(Clone, Debug, PartialEq)]
pub struct ShardHeader {
    /// 分片序号（从 0 开始，必须连续）。
    pub shard: usize,
    /// 本分片第一条记录的全局序号。
    pub first_seq: usize,
    /// 本分片记录条数。
    pub count: usize,
    /// 父分片的链头（第一片是 [`GENESIS`]）。
    pub prev: u64,
    /// 本分片的链头。
    pub head: u64,
    /// 分片内验证集表（有效表 = 父表 + 本片新增，按首次出现排序）。
    pub oracles: Vec<Value>,
    /// 复用哪一片的 oracle 表（`Some(p)` 表示 `oracles` 的前 `p` 片部分继承而来）。
    pub oracle_parent: Option<usize>,
    /// 本片**新增**验证集的摘要（不含继承部分）。
    pub oracles_digest: u64,
}

impl ShardHeader {
    /// 新增验证集表的摘要（表内容被改动时对不上）。
    #[must_use]
    pub fn digest(&self) -> u64 {
        self.oracles_digest
    }
}

/// 一个分片：header + 记录。
#[derive(Clone, Debug, PartialEq)]
pub struct Shard {
    header: ShardHeader,
    entries: Vec<LogEntry>,
}

impl Shard {
    /// 分片头。
    #[must_use]
    pub fn header(&self) -> &ShardHeader {
        &self.header
    }

    /// 本分片记录（`seq` 是全局序号）。
    #[must_use]
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }
}

/// 分片后的审计日志。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShardedLog {
    shards: Vec<Shard>,
}

impl AuditLog {
    /// 按条数切分（先验证整条链；空日志得到 0 个分片）。
    pub fn into_shards(self, max_entries: usize) -> Result<ShardedLog> {
        if max_entries == 0 {
            return Err(Error::Audit("分片大小必须大于 0".into()));
        }
        self.verify_chain()?;
        let entries = self.entries().to_vec();
        let mut shards: Vec<Shard> = Vec::new();
        let mut offset = 0usize;
        let mut prev = GENESIS;
        let mut inherited: Vec<Value> = Vec::new();
        while offset < entries.len() {
            let end = (offset + max_entries).min(entries.len());
            let chunk = &entries[offset..end];
            let local: Vec<Value> = oracle_table(chunk)
                .into_iter()
                .filter(|oracle| !inherited.contains(oracle))
                .collect();
            let digest = oracles_digest(&local);
            let mut oracles = inherited.clone();
            oracles.extend(local);
            let head = chunk.last().map_or(prev, |entry| entry.chain);
            let parent = if shards.is_empty() {
                None
            } else {
                Some(shards.len() - 1)
            };
            shards.push(Shard {
                header: ShardHeader {
                    shard: shards.len(),
                    first_seq: offset,
                    count: chunk.len(),
                    prev,
                    head,
                    oracles: oracles.clone(),
                    oracle_parent: parent,
                    oracles_digest: digest,
                },
                entries: chunk.to_vec(),
            });
            inherited = oracles;
            prev = head;
            offset = end;
        }
        Ok(ShardedLog { shards })
    }
}

impl ShardedLog {
    /// 空分片集。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 分片列表。
    #[must_use]
    pub fn shards(&self) -> &[Shard] {
        &self.shards
    }

    /// 分片数。
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.shards.len()
    }

    /// 记录总数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.entries.len()).sum()
    }

    /// 是否没有记录。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 整条链的链头（空日志返回 [`GENESIS`]）。
    #[must_use]
    pub fn head(&self) -> u64 {
        self.shards
            .last()
            .map_or(GENESIS, |shard| shard.header.head)
    }

    /// 序列化为分片文本（与 [`ShardedLog::from_texts`] 互逆）。
    #[must_use]
    pub fn to_texts(&self) -> Vec<String> {
        self.shards
            .iter()
            .enumerate()
            .map(|(index, shard)| shard_text(self, index, shard))
            .collect()
    }

    /// 从分片文本解析；不自动验链（调用方显式调 [`ShardedLog::verify_chain`]）。
    ///
    /// 每个分片首行必须是 header，其后是 v1 记录；记录只能通过 `oracle_index`
    /// 引用本分片的 oracle 表。
    pub fn from_texts(texts: &[String]) -> Result<Self> {
        let mut shards = Vec::with_capacity(texts.len());
        let mut inherited: Vec<Value> = Vec::new();
        for (index, text) in texts.iter().enumerate() {
            let shard = parse_shard(text, index, &inherited)?;
            inherited = shard.header.oracles.clone();
            shards.push(shard);
        }
        Ok(Self { shards })
    }

    /// 跨分片校验：序号连续、`prev` / `head` 衔接、逐条 `chain` 可重算。
    pub fn verify_chain(&self) -> Result<()> {
        let mut prev = GENESIS;
        let mut seq = 0usize;
        for (index, shard) in self.shards.iter().enumerate() {
            let parent = index
                .checked_sub(1)
                .and_then(|previous| self.shards.get(previous))
                .map(|previous| previous.header.oracles.as_slice());
            verify_shard(shard, index, &mut prev, &mut seq, parent)?;
        }
        Ok(())
    }

    /// 从 `initial` 重放全部分片。
    pub fn replay(&self, initial: &Net) -> Result<ReplayReport> {
        self.replay_net(initial).map(|(report, _)| report)
    }

    /// 重放并返回最终工件。
    pub fn replay_net(&self, initial: &Net) -> Result<(ReplayReport, Net)> {
        self.verify_chain()?;
        let entries: Vec<LogEntry> = self
            .shards
            .iter()
            .flat_map(|shard| shard.entries.iter().cloned())
            .collect();
        replay_entries(&entries, initial)
    }
}

/// 流式分片游标：逐个 `push` 解析并验链，不保留已读分片。
#[derive(Clone, Debug, Default)]
pub struct ShardCursor {
    shards: usize,
    prev: u64,
    seq: usize,
    table: Vec<Value>,
}

/// 分片重放的检查点：下一个分片号、已消费条数、链头与当前工件。
#[derive(Clone, Debug, PartialEq)]
pub struct Checkpoint {
    /// 下一个要读的分片号。
    pub shard: usize,
    /// 已消费的记录数。
    pub entries: usize,
    /// 已消费部分的链头。
    pub head: u64,
    /// 分片边界处的工件。
    pub net: Net,
    /// 已应用记录的计数。
    pub applied: usize,
    /// 已回滚记录的计数。
    pub rolled_back: usize,
    /// 未应用（前置不成立）记录的计数。
    pub skipped: usize,
    /// 已核对行为指纹的记录数。
    pub behavior_checked: usize,
    /// 检查点处的有效 oracle 表（续放时新分片可能引用继承项）。
    pub oracles: Vec<Value>,
}

impl ShardCursor {
    /// 从检查点继续：下一次 `push` 必须是 `shard` 号分片。
    #[must_use]
    pub fn resume(shard: usize, entries: usize, head: u64, table: Vec<Value>) -> Self {
        Self {
            shards: shard,
            prev: head,
            seq: entries,
            table,
        }
    }

    /// 从第一个分片开始。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 读入下一个分片（必须是下一序号），校验并返回；调用方可以随即丢弃。
    pub fn push(&mut self, text: &str) -> Result<Shard> {
        let shard = parse_shard(text, self.shards, &self.table)?;
        let parent = if self.shards == 0 {
            None
        } else {
            Some(self.table.as_slice())
        };
        verify_shard(&shard, self.shards, &mut self.prev, &mut self.seq, parent)?;
        self.table = shard.header.oracles.clone();
        self.shards += 1;
        Ok(shard)
    }

    /// 已读分片数。
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.shards
    }

    /// 已读记录数。
    #[must_use]
    pub fn entries(&self) -> usize {
        self.seq
    }

    /// 当前链头。
    #[must_use]
    pub fn head(&self) -> u64 {
        self.prev
    }
}

/// 流式跨片重放：读一片、校验一片、应用一片，不保留分片文本。
pub struct ShardReplay {
    cursor: ShardCursor,
    state: ReplayState,
}

impl ShardReplay {
    /// 从初始工件开始。
    #[must_use]
    pub fn new(initial: &Net) -> Self {
        Self {
            cursor: ShardCursor::new(),
            state: ReplayState::new(initial),
        }
    }

    /// 读入并应用下一个分片。
    pub fn push(&mut self, text: &str) -> Result<()> {
        let shard = self.cursor.push(text)?;
        for entry in shard.entries() {
            self.state.apply_entry(entry)?;
        }
        Ok(())
    }

    /// 已读分片数。
    #[must_use]
    pub fn shard_count(&self) -> usize {
        self.cursor.shard_count()
    }

    /// 已应用记录数。
    #[must_use]
    pub fn entries(&self) -> usize {
        self.cursor.entries()
    }

    /// 当前链头。
    #[must_use]
    pub fn head(&self) -> u64 {
        self.cursor.head()
    }

    /// 当前工件。
    #[must_use]
    pub fn net(&self) -> &Net {
        self.state.net()
    }

    /// 在当前位置取检查点（工件克隆一份）。
    #[must_use]
    pub fn checkpoint(&self) -> Checkpoint {
        let (applied, rolled_back, skipped, behavior_checked) = self.state.counters();
        Checkpoint {
            shard: self.cursor.shard_count(),
            entries: self.cursor.entries(),
            head: self.cursor.head(),
            net: self.state.net().clone(),
            applied,
            rolled_back,
            skipped,
            behavior_checked,
            oracles: self.cursor.table.clone(),
        }
    }

    /// 从检查点继续（后续分片仍逐片验链）。
    #[must_use]
    pub fn resume(checkpoint: &Checkpoint) -> Self {
        Self {
            cursor: ShardCursor::resume(
                checkpoint.shard,
                checkpoint.entries,
                checkpoint.head,
                checkpoint.oracles.clone(),
            ),
            state: ReplayState::resume(
                &checkpoint.net,
                checkpoint.entries,
                checkpoint.applied,
                checkpoint.rolled_back,
                checkpoint.skipped,
                checkpoint.behavior_checked,
            ),
        }
    }

    /// 结束并给出重放报告与最终工件。
    #[must_use]
    pub fn finish(self) -> (ReplayReport, Net) {
        self.state.finish()
    }
}

fn verify_shard(
    shard: &Shard,
    index: usize,
    prev: &mut u64,
    seq: &mut usize,
    parent: Option<&[Value]>,
) -> Result<()> {
    let header = &shard.header;
    if header.shard != index {
        return Err(Error::Audit(format!(
            "第 {index} 个分片头写的是 {}",
            header.shard
        )));
    }
    if header.first_seq != *seq {
        return Err(Error::Audit(format!(
            "分片 {index} first_seq={}（全局序号应为 {seq}）",
            header.first_seq
        )));
    }
    if header.prev != *prev {
        return Err(Error::Audit(format!(
            "分片 {index} prev={:016x}，应为 {:016x}",
            header.prev, *prev
        )));
    }
    let local: &[Value] = match header.oracle_parent {
        Some(parent_index) => {
            if parent_index + 1 != index {
                return Err(Error::Audit(format!(
                    "分片 {index} 的 oracle_parent={parent_index}（应为上一片）"
                )));
            }
            let Some(base) = parent else {
                return Err(Error::Audit(format!(
                    "分片 {index} 引用了父表，但没有父分片上下文"
                )));
            };
            if !header.oracles.starts_with(base) {
                return Err(Error::Audit(format!(
                    "分片 {index} 的 oracle 表不以父表开头"
                )));
            }
            &header.oracles[base.len()..]
        }
        None => &header.oracles,
    };
    let want = oracles_digest(local);
    if header.oracles_digest != want {
        return Err(Error::Audit(format!(
            "分片 {index} 的 oracle 表摘要={:016x}，重算 {want:016x}",
            header.oracles_digest
        )));
    }
    if header.count != shard.entries.len() {
        return Err(Error::Audit(format!(
            "分片 {index} count={}，实际 {} 条",
            header.count,
            shard.entries.len()
        )));
    }
    if shard.entries.is_empty() {
        return Err(Error::Audit(format!("分片 {index} 是空的")));
    }
    for entry in &shard.entries {
        if entry.seq != *seq {
            return Err(Error::Audit(format!(
                "分片 {index} 中序号是 {}（应为 {seq}）",
                entry.seq
            )));
        }
        if entry.prev != *prev {
            return Err(Error::Audit(format!(
                "第 {seq} 条 prev={:016x}，应为 {:016x}",
                entry.prev, *prev
            )));
        }
        let want = chain_of(entry);
        if entry.chain != want {
            return Err(Error::Audit(format!(
                "第 {seq} 条 chain={:016x}，重算 {:016x}（内容被改过）",
                entry.chain, want
            )));
        }
        *prev = entry.chain;
        *seq += 1;
    }
    if header.head != *prev {
        return Err(Error::Audit(format!(
            "分片 {index} head={:016x}，实际 {:016x}",
            header.head, *prev
        )));
    }
    Ok(())
}

fn shard_text(log: &ShardedLog, index: usize, shard: &Shard) -> String {
    let header = &shard.header;
    let local: Vec<Value> = match header.oracle_parent {
        Some(parent_index) => {
            let parent = &log.shards[parent_index].header.oracles;
            if header.oracles.starts_with(parent) {
                header.oracles[parent.len()..].to_vec()
            } else {
                header.oracles.clone()
            }
        }
        None => header.oracles.clone(),
    };
    let digest = oracles_digest(&local);
    let parent_value = header
        .oracle_parent
        .map_or(Value::Null, |parent_index| json!(parent_index));
    let mut out = json!({
        "v": SHARD_VERSION,
        "kind": SHARD_KIND,
        "shard": index,
        "first_seq": header.first_seq,
        "count": header.count,
        "prev": hex(header.prev),
        "head": hex(header.head),
        "oracle_parent": parent_value,
        "oracles": local,
        "oracles_digest": hex(digest),
    })
    .to_string();
    out.push('\n');
    for entry in &shard.entries {
        let mut value = canonical(entry, true);
        crate::history_time::attach(&mut value, entry.timestamp);
        if let Some(oracle) = &entry.oracle {
            let index = header
                .oracles
                .iter()
                .position(|candidate| candidate == oracle)
                .expect("oracle table built from entries");
            let map = value.as_object_mut().expect("entry is an object");
            map.remove("oracle");
            map.insert("oracle_index".to_string(), json!(index));
        }
        out.push_str(&value.to_string());
        out.push('\n');
    }
    out
}

fn parse_shard(text: &str, expected: usize, inherited: &[Value]) -> Result<Shard> {
    let mut lines = text.lines().filter(|line| !line.trim().is_empty());
    let raw_header = lines
        .next()
        .ok_or_else(|| Error::Audit(format!("分片 {expected} 是空的")))?;
    let header_value: Value = serde_json::from_str(raw_header)
        .map_err(|error| Error::Audit(format!("分片 {expected} 的 header JSON：{error}")))?;
    let version = header_value
        .get("v")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Audit(format!("分片 {expected} 的 header 缺少 v")))?;
    if version != SHARD_VERSION {
        return Err(Error::Audit(format!(
            "分片 {expected} 的 header 版本 {version}（当前 {SHARD_VERSION}）"
        )));
    }
    let kind = header_value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("");
    if kind != SHARD_KIND {
        return Err(Error::Audit(format!(
            "分片 {expected} 的 kind 是 `{kind}`（应为 `{SHARD_KIND}`）"
        )));
    }
    let shard = usize_field(&header_value, "shard", expected)?;
    if shard != expected {
        return Err(Error::Audit(format!(
            "第 {expected} 个分片头写的是 {shard}"
        )));
    }
    let first_seq = usize_field(&header_value, "first_seq", expected)?;
    let count = usize_field(&header_value, "count", expected)?;
    let prev = hex_field(&header_value, "prev", expected)?;
    let head = hex_field(&header_value, "head", expected)?;
    let parent = optional_usize_field(&header_value, "oracle_parent", expected)?;
    let local = header_value
        .get("oracles")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::Audit(format!("分片 {expected} 缺少 oracles 数组")))?
        .clone();
    for oracle in &local {
        wire::oracle_from_value(oracle)?;
    }
    for (left, value) in local.iter().enumerate() {
        if local[..left].contains(value) {
            return Err(Error::Audit(format!(
                "分片 {expected} 的 oracle 表第 {left} 项重复"
            )));
        }
    }
    let oracles = if let Some(parent_index) = parent {
        if parent_index + 1 != expected {
            return Err(Error::Audit(format!(
                "分片 {expected} 的 oracle_parent={parent_index}（应为上一片）"
            )));
        }
        let mut effective = inherited.to_vec();
        for value in &local {
            if effective.contains(value) {
                return Err(Error::Audit(format!(
                    "分片 {expected} 的增量表重复了父表里的验证集"
                )));
            }
            effective.push(value.clone());
        }
        effective
    } else {
        local.clone()
    };
    let digest = hex_field(&header_value, "oracles_digest", expected)?;
    let want_digest = oracles_digest(&local);
    if digest != want_digest {
        return Err(Error::Audit(format!(
            "分片 {expected} 的 oracle 表摘要={digest:016x}，重算 {want_digest:016x}"
        )));
    }

    let parent_len = oracles.len().saturating_sub(local.len());
    let mut entries = Vec::with_capacity(count);
    let mut used = vec![false; local.len()];
    for (line, raw) in lines.enumerate() {
        let value: Value = serde_json::from_str(raw).map_err(|error| {
            Error::Audit(format!("分片 {expected} 第 {} 行 JSON：{error}", line + 2))
        })?;
        let parts = parse_entry(&value, line + 2)?;
        if parts.inline_oracle.is_some() {
            return Err(Error::Audit(format!(
                "分片 {expected} 第 {} 行内嵌了验证集（应使用 oracle_index）",
                line + 2
            )));
        }
        let oracle = match parts.oracle_index {
            Some(index) => {
                let value = oracles.get(index).ok_or_else(|| {
                    Error::Audit(format!(
                        "分片 {expected} 第 {} 行 oracle_index={index} 越界（表长 {}）",
                        line + 2,
                        oracles.len()
                    ))
                })?;
                if index >= parent_len {
                    used[index - parent_len] = true;
                }
                Some(value.clone())
            }
            None if parts.oracle_ref.is_some() => {
                return Err(Error::Audit(format!(
                    "分片 {expected} 第 {} 行有 oracle_ref 但没有 oracle_index",
                    line + 2
                )));
            }
            None => None,
        };
        let mut entry = parts.entry;
        entry.oracle = oracle;
        entries.push(entry);
    }
    if entries.len() != count {
        return Err(Error::Audit(format!(
            "分片 {expected} count={count}，实际 {} 条",
            entries.len()
        )));
    }
    if let Some(position) = used.iter().position(|used| !used) {
        return Err(Error::Audit(format!(
            "分片 {expected} 的 oracle 表第 {position} 项没有被引用"
        )));
    }
    Ok(Shard {
        header: ShardHeader {
            shard,
            first_seq,
            count,
            prev,
            head,
            oracles,
            oracle_parent: parent,
            oracles_digest: digest,
        },
        entries,
    })
}

fn optional_usize_field(value: &Value, name: &str, shard: usize) -> Result<Option<usize>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => {
            let number = raw
                .as_u64()
                .ok_or_else(|| Error::Audit(format!("分片 {shard} 的 `{name}` 必须是非负整数")))?;
            usize::try_from(number)
                .map(Some)
                .map_err(|_| Error::Audit(format!("分片 {shard} 的 `{name}` 超出 usize")))
        }
    }
}

fn oracle_table(entries: &[LogEntry]) -> Vec<Value> {
    let mut table: Vec<Value> = Vec::new();
    for entry in entries {
        if let Some(oracle) = &entry.oracle
            && !table.contains(oracle)
        {
            table.push(oracle.clone());
        }
    }
    table
}

fn oracles_digest(oracles: &[Value]) -> u64 {
    let mut bytes = Vec::new();
    for oracle in oracles {
        bytes.extend_from_slice(oracle.to_string().as_bytes());
        bytes.push(b'\n');
    }
    fnv(&bytes)
}

fn hex_field(value: &Value, name: &str, shard: usize) -> Result<u64> {
    let text = value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Audit(format!("分片 {shard} 的 header 缺少 `{name}`")))?;
    u64::from_str_radix(text, 16)
        .map_err(|_| Error::Audit(format!("分片 {shard} 的 `{name}` 不是十六进制：{text}")))
}

fn usize_field(value: &Value, name: &str, shard: usize) -> Result<usize> {
    let number = value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Audit(format!("分片 {shard} 的 header 缺少 `{name}`")))?;
    usize::try_from(number)
        .map_err(|_| Error::Audit(format!("分片 {shard} 的 `{name}` 超出 usize")))
}
