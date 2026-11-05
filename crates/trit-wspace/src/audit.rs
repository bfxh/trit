//! 可重放的编辑审计日志：JSONL + 哈希链。
//!
//! 每一行是一条编辑记录：前/后工件校验和、补丁、改动量、应用/回滚标志，
//! 以及 `prev` / `chain` 两个哈希链字段。`verify_chain` 抓篡改与乱序；
//! `replay` 从初始工件逐条重放，校验每一步的 `before` / `after` 校验和。
//!
//! 记录时如果给了验证集，还会存下验证集规范 + 前/后**行为指纹**；重放会同时
//! 核对“权重链”和“行为链”。回滚条目必须前后同校验和、同行为指纹；
//! 未应用条目（前置不成立）同理。三条不变量让日志本身自洽。

use serde_json::{Value, json};

use crate::codec::checksum;
use crate::edit::Patch;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::{Behavior, Oracle};
use crate::wire;

/// 日志线格式版本。
pub const LOG_VERSION: u64 = 1;

/// 创世链值。
pub const GENESIS: u64 = 0;

/// 单条记录最多存多大的验证集（超过就报错，避免日志爆掉）。
pub const MAX_LOGGED_ORACLE: usize = 4096;

/// 一条编辑记录。
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// 序号（从 0 开始，必须连续）。
    pub seq: usize,
    /// 记录名（例如 `repair-xor`）。
    pub name: String,
    /// 应用前工件校验和。
    pub before: u64,
    /// 应用后工件校验和（回滚/未应用时等于 `before`）。
    pub after: u64,
    /// 补丁是否被应用过。
    pub applied: bool,
    /// 是否因后置失败而回滚。
    pub rolled_back: bool,
    /// 改动量 JSON（来自 [`wire::cost_to_value`]）。
    pub cost: Value,
    /// 实际补丁。
    pub patch: Patch,
    /// 记录时使用的验证集（可选；紧凑 `inputs` / `outputs` JSON）。
    ///
    /// 即使本条只写了 `oracle_ref`，解析时也会解析成完整值放在这里，供重放核对。
    pub oracle: Option<Value>,
    /// 同一验证集首次出现的那条记录的序号；`None` 表示本条自己内嵌了验证集。
    pub oracle_ref: Option<usize>,
    /// 验证集上的行为指纹（可选）。
    pub behavior_before: Option<u64>,
    /// 应用后的行为指纹（可选；回滚/未应用时等于 `behavior_before`）。
    pub behavior_after: Option<u64>,
    /// 上一条的 `chain`；第一条是 [`GENESIS`]。
    pub prev: u64,
    /// 本条哈希链值。
    pub chain: u64,
}

/// 重放结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplayReport {
    /// 记录条数。
    pub entries: usize,
    /// 实际应用到工件上的条数。
    pub applied: usize,
    /// 应用后又回滚的条数。
    pub rolled_back: usize,
    /// 未应用（前置不成立）的条数。
    pub skipped: usize,
    /// 实际核对了行为指纹的条数。
    pub behavior_checked: usize,
    /// 最终工件的校验和。
    pub final_checksum: u64,
}

/// 审计日志。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AuditLog {
    entries: Vec<LogEntry>,
}

impl AuditLog {
    /// 空日志。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 全部记录。
    #[must_use]
    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    /// 记录条数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否空日志。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 链头（空日志返回 [`GENESIS`]）。
    #[must_use]
    pub fn head(&self) -> u64 {
        self.entries.last().map_or(GENESIS, |entry| entry.chain)
    }

    /// 追加一条记录（不带验证集）。
    pub fn record(
        &mut self,
        name: impl Into<String>,
        before: &Net,
        after: &Net,
        patch: Patch,
        applied: bool,
        rolled_back: bool,
    ) -> Result<()> {
        self.record_with_oracle(name, before, after, patch, applied, rolled_back, None)
    }

    /// 追加一条记录，可选存下验证集与前后行为指纹。
    ///
    /// 不变量：未应用 / 回滚的条目必须前后校验和相同（工件没被留在半改状态），
    /// 且行为指纹相同。
    #[allow(clippy::too_many_arguments)]
    pub fn record_with_oracle(
        &mut self,
        name: impl Into<String>,
        before: &Net,
        after: &Net,
        patch: Patch,
        applied: bool,
        rolled_back: bool,
        oracle: Option<&Oracle>,
    ) -> Result<()> {
        if before.arch() != after.arch() {
            return Err(Error::ArchMismatch);
        }
        let before_sum = checksum(before);
        let after_sum = checksum(after);
        if !applied && before_sum != after_sum {
            return Err(Error::Audit("未应用补丁但前后工件校验和不同".into()));
        }
        if rolled_back && before_sum != after_sum {
            return Err(Error::Audit("回滚条目前后工件校验和不同".into()));
        }
        let (oracle_value, oracle_ref, behavior_before, behavior_after) = if let Some(oracle) =
            oracle
        {
            if oracle.len() > MAX_LOGGED_ORACLE {
                return Err(Error::Audit(format!(
                    "验证集 {} 条超过日志上限 {MAX_LOGGED_ORACLE}",
                    oracle.len()
                )));
            }
            let before_sig = Behavior::of(before, oracle)?.signature();
            let after_sig = Behavior::of(after, oracle)?.signature();
            if (!applied || rolled_back) && before_sig != after_sig {
                return Err(Error::Audit("回滚/未应用条目的行为指纹发生变化".into()));
            }
            let value = wire::oracle_to_value(oracle);
            let root = self.entries.iter().enumerate().find_map(|(index, entry)| {
                (entry.oracle.as_ref() == Some(&value)).then(|| entry.oracle_ref.unwrap_or(index))
            });
            (Some(value), root, Some(before_sig), Some(after_sig))
        } else {
            (None, None, None, None)
        };
        let seq = self.entries.len();
        let prev = self.head();
        let cost = wire::cost_to_value(&patch.cost(before));
        let mut entry = LogEntry {
            seq,
            name: name.into(),
            before: before_sum,
            after: after_sum,
            applied,
            rolled_back,
            cost,
            patch,
            oracle: oracle_value,
            oracle_ref,
            behavior_before,
            behavior_after,
            prev,
            chain: 0,
        };
        entry.chain = chain_of(&entry);
        self.entries.push(entry);
        Ok(())
    }

    /// 校验序号连续、`prev` 衔接、每条 `chain` 可重算。
    pub fn verify_chain(&self) -> Result<()> {
        let mut prev = GENESIS;
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.seq != index {
                return Err(Error::Audit(format!(
                    "第 {index} 条序号是 {}（应为 {index}）",
                    entry.seq
                )));
            }
            if entry.prev != prev {
                return Err(Error::Audit(format!(
                    "第 {index} 条 prev={:016x}，应为 {:016x}",
                    entry.prev, prev
                )));
            }
            let want = chain_of(entry);
            if entry.chain != want {
                return Err(Error::Audit(format!(
                    "第 {index} 条 chain={:016x}，重算 {:016x}（内容被改过）",
                    entry.chain, want
                )));
            }
            prev = entry.chain;
        }
        Ok(())
    }

    /// 重算整条链（日志合并/手工修订后使用；不验证已有内容）。
    pub fn recompute_chain(&mut self) {
        let mut prev = GENESIS;
        for entry in &mut self.entries {
            entry.prev = prev;
            entry.chain = chain_of(entry);
            prev = entry.chain;
        }
    }

    /// 从 `initial` 重放：逐条校验 before/after 校验和。
    pub fn replay(&self, initial: &Net) -> Result<ReplayReport> {
        self.replay_net(initial).map(|(report, _)| report)
    }

    /// 重放并返回最终工件；有验证集的条目会同时核对前后行为指纹。
    pub fn replay_net(&self, initial: &Net) -> Result<(ReplayReport, Net)> {
        self.verify_chain()?;
        replay_entries(&self.entries, initial)
    }

    /// 序列化为 JSONL（一行一条记录）。
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            out.push_str(&canonical(entry, true).to_string());
            out.push('\n');
        }
        out
    }

    /// 解析 JSONL；空行忽略。不自动验链（调用方显式调 [`AuditLog::verify_chain`]）。
    pub fn from_jsonl(text: &str) -> Result<Self> {
        let mut log = Self::default();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let value: Value = serde_json::from_str(line)
                .map_err(|error| Error::Audit(format!("第 {} 行 JSON：{error}", index + 1)))?;
            let parts = parse_entry(&value, index + 1)?;
            if parts.oracle_index.is_some() {
                return Err(Error::Audit(format!(
                    "第 {} 行有 oracle_index（只用于分片文件）",
                    index + 1
                )));
            }
            let oracle = match (parts.inline_oracle, parts.oracle_ref) {
                (Some(_), Some(_)) => {
                    return Err(Error::Audit(format!(
                        "第 {} 行同时有 oracle 与 oracle_ref",
                        index + 1
                    )));
                }
                (Some(oracle), None) => Some(oracle),
                (None, Some(root)) => {
                    let Some(entry) = log.entries.get(root) else {
                        return Err(Error::Audit(format!(
                            "第 {} 行 oracle_ref={root} 指向不存在的条目（只能向前引用）",
                            index + 1
                        )));
                    };
                    Some(entry.oracle.clone().ok_or_else(|| {
                        Error::Audit(format!(
                            "第 {} 行 oracle_ref={root} 指向没有验证集的条目",
                            index + 1
                        ))
                    })?)
                }
                (None, None) => None,
            };
            let mut entry = parts.entry;
            entry.oracle = oracle;
            log.entries.push(entry);
        }
        Ok(log)
    }
}

/// 解析一行记录，验证集字段留待调用方解析（普通日志 / 分片表）。
pub(crate) struct EntryParts {
    pub(crate) entry: LogEntry,
    pub(crate) inline_oracle: Option<Value>,
    pub(crate) oracle_ref: Option<usize>,
    pub(crate) oracle_index: Option<usize>,
}

pub(crate) fn parse_entry(value: &Value, line: usize) -> Result<EntryParts> {
    let version = value
        .get("v")
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Audit(format!("第 {line} 行缺少 v")))?;
    if version != LOG_VERSION {
        return Err(Error::Audit(format!(
            "第 {line} 行版本 {version}（当前 {LOG_VERSION}）"
        )));
    }
    let patch_value = value
        .get("patch")
        .ok_or_else(|| Error::Audit(format!("第 {line} 行缺少 patch")))?;
    let patch = wire::patch_from_value(patch_value)?;
    let inline_oracle = parse_oracle(value)?;
    let oracle_ref = optional_usize_field(value, "oracle_ref")?;
    let oracle_index = optional_usize_field(value, "oracle_index")?;
    let entry = LogEntry {
        seq: usize_field(value, "seq")?,
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        before: hex_field(value, "before")?,
        after: hex_field(value, "after")?,
        applied: value
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        rolled_back: value
            .get("rolled_back")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        cost: value.get("cost").cloned().unwrap_or_else(|| json!({})),
        patch,
        oracle: None,
        oracle_ref,
        behavior_before: optional_hex_field(value, "behavior_before")?,
        behavior_after: optional_hex_field(value, "behavior_after")?,
        prev: hex_field(value, "prev")?,
        chain: hex_field(value, "chain")?,
    };
    Ok(EntryParts {
        entry,
        inline_oracle,
        oracle_ref,
        oracle_index,
    })
}

pub(crate) struct ReplayState {
    current: Net,
    entries: usize,
    applied: usize,
    rolled_back: usize,
    skipped: usize,
    behavior_checked: usize,
}

impl ReplayState {
    pub(crate) fn new(initial: &Net) -> Self {
        Self::resume(initial, 0, 0, 0, 0, 0)
    }

    pub(crate) fn resume(
        initial: &Net,
        entries: usize,
        applied: usize,
        rolled_back: usize,
        skipped: usize,
        behavior_checked: usize,
    ) -> Self {
        Self {
            current: initial.clone(),
            entries,
            applied,
            rolled_back,
            skipped,
            behavior_checked,
        }
    }

    pub(crate) fn counters(&self) -> (usize, usize, usize, usize) {
        (
            self.applied,
            self.rolled_back,
            self.skipped,
            self.behavior_checked,
        )
    }

    pub(crate) fn apply_entry(&mut self, entry: &LogEntry) -> Result<()> {
        let oracle = match &entry.oracle {
            Some(value) => Some(wire::oracle_from_value(value)?),
            None => None,
        };
        if oracle.is_none() && (entry.behavior_before.is_some() || entry.behavior_after.is_some()) {
            return Err(Error::Audit(format!(
                "第 {} 条有行为指纹但没有验证集",
                entry.seq
            )));
        }
        if let Some(oracle) = &oracle {
            let sig = Behavior::of(&self.current, oracle)?.signature();
            if Some(sig) != entry.behavior_before {
                return Err(Error::Audit(format!(
                    "第 {} 条 behavior_before 不符（记录 {:?}，重算 {:016x}）",
                    entry.seq, entry.behavior_before, sig
                )));
            }
        }
        let now = checksum(&self.current);
        if now != entry.before {
            return Err(Error::Audit(format!(
                "第 {} 条 before={:016x}，当前工件 {:016x}",
                entry.seq, entry.before, now
            )));
        }
        if !entry.applied {
            self.skipped += 1;
        } else {
            let journal = entry.patch.apply(&mut self.current)?;
            if entry.rolled_back {
                journal.undo(&mut self.current);
                self.rolled_back += 1;
            } else {
                self.applied += 1;
            }
        }
        let after = checksum(&self.current);
        if after != entry.after {
            return Err(Error::Audit(format!(
                "第 {} 条 after={:016x}，重放得到 {:016x}",
                entry.seq, entry.after, after
            )));
        }
        if let Some(oracle) = &oracle {
            let sig = Behavior::of(&self.current, oracle)?.signature();
            if Some(sig) != entry.behavior_after {
                return Err(Error::Audit(format!(
                    "第 {} 条 behavior_after 不符（记录 {:?}，重算 {:016x}）",
                    entry.seq, entry.behavior_after, sig
                )));
            }
            self.behavior_checked += 1;
        }
        self.entries += 1;
        Ok(())
    }

    pub(crate) fn net(&self) -> &Net {
        &self.current
    }

    pub(crate) fn finish(self) -> (ReplayReport, Net) {
        let report = ReplayReport {
            entries: self.entries,
            applied: self.applied,
            rolled_back: self.rolled_back,
            skipped: self.skipped,
            behavior_checked: self.behavior_checked,
            final_checksum: checksum(&self.current),
        };
        (report, self.current)
    }
}

pub(crate) fn replay_entries(entries: &[LogEntry], initial: &Net) -> Result<(ReplayReport, Net)> {
    let mut state = ReplayState::new(initial);
    for entry in entries {
        state.apply_entry(entry)?;
    }
    Ok(state.finish())
}

pub(crate) fn chain_of(entry: &LogEntry) -> u64 {
    fnv(canonical(entry, false).to_string().as_bytes())
}

pub(crate) fn canonical(entry: &LogEntry, with_chain: bool) -> Value {
    let mut map = serde_json::Map::new();
    map.insert("v".to_string(), json!(LOG_VERSION));
    map.insert("seq".to_string(), json!(entry.seq));
    map.insert("name".to_string(), json!(entry.name));
    map.insert("before".to_string(), json!(hex(entry.before)));
    map.insert("after".to_string(), json!(hex(entry.after)));
    map.insert("applied".to_string(), json!(entry.applied));
    map.insert("rolled_back".to_string(), json!(entry.rolled_back));
    map.insert("cost".to_string(), entry.cost.clone());
    map.insert("patch".to_string(), wire::patch_to_value(&entry.patch));
    map.insert("prev".to_string(), json!(hex(entry.prev)));
    match (&entry.oracle, entry.oracle_ref) {
        (Some(_), Some(root)) => {
            map.insert("oracle_ref".to_string(), json!(root));
        }
        (Some(oracle), None) => {
            map.insert("oracle".to_string(), oracle.clone());
        }
        _ => {}
    }
    if let Some(sig) = entry.behavior_before {
        map.insert("behavior_before".to_string(), json!(hex(sig)));
    }
    if let Some(sig) = entry.behavior_after {
        map.insert("behavior_after".to_string(), json!(hex(sig)));
    }
    if with_chain {
        map.insert("chain".to_string(), json!(hex(entry.chain)));
    }
    Value::Object(map)
}

pub(crate) fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

pub(crate) fn hex(value: u64) -> String {
    format!("{value:016x}")
}

fn hex_field(value: &Value, name: &str) -> Result<u64> {
    let text = value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Audit(format!("缺少十六进制字段 `{name}`")))?;
    u64::from_str_radix(text, 16)
        .map_err(|_| Error::Audit(format!("`{name}` 不是 16 位十六进制：{text}")))
}

fn optional_hex_field(value: &Value, name: &str) -> Result<Option<u64>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => u64::from_str_radix(text, 16)
            .map(Some)
            .map_err(|_| Error::Audit(format!("`{name}` 不是十六进制：{text}"))),
        Some(_) => Err(Error::Audit(format!("`{name}` 必须是字符串"))),
    }
}

fn optional_usize_field(value: &Value, name: &str) -> Result<Option<usize>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => {
            let number = raw
                .as_u64()
                .ok_or_else(|| Error::Audit(format!("`{name}` 必须是非负整数")))?;
            usize::try_from(number)
                .map(Some)
                .map_err(|_| Error::Audit(format!("`{name}` 超出 usize")))
        }
    }
}

fn parse_oracle(value: &Value) -> Result<Option<Value>> {
    match value.get("oracle") {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => {
            wire::oracle_from_value(raw)?;
            Ok(Some(raw.clone()))
        }
    }
}

pub(crate) fn usize_field(value: &Value, name: &str) -> Result<usize> {
    let number = value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Audit(format!("缺少整数字段 `{name}`")))?;
    usize::try_from(number).map_err(|_| Error::Audit(format!("`{name}` 超出 usize")))
}
