//! 可重放的编辑审计日志：JSONL + 哈希链。
//!
//! 每一行是一条编辑记录：前/后工件校验和、补丁、改动量、应用/回滚标志，
//! 以及 `prev` / `chain` 两个哈希链字段。`verify_chain` 抓篡改与乱序；
//! `replay` 从初始工件逐条重放，校验每一步的 `before` / `after` 校验和。
//!
//! 回滚条目必须前后同校验和；未应用条目（前置不成立）也必须前后同校验和。
//! 这两条让日志本身自洽，重放时不需要猜“这一步到底有没有改工件”。

use serde_json::{Value, json};

use crate::codec::checksum;
use crate::edit::Patch;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::wire;

/// 日志线格式版本。
pub const LOG_VERSION: u64 = 1;

/// 创世链值。
pub const GENESIS: u64 = 0;

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

    /// 追加一条记录。
    ///
    /// 不变量：未应用 / 回滚的条目必须前后校验和相同（工件没被留在半改状态）。
    pub fn record(
        &mut self,
        name: impl Into<String>,
        before: &Net,
        after: &Net,
        patch: Patch,
        applied: bool,
        rolled_back: bool,
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

    /// 从 `initial` 重放：逐条校验 before/after 校验和。
    pub fn replay(&self, initial: &Net) -> Result<ReplayReport> {
        self.replay_net(initial).map(|(report, _)| report)
    }

    /// 重放并返回最终工件。
    pub fn replay_net(&self, initial: &Net) -> Result<(ReplayReport, Net)> {
        self.verify_chain()?;
        let mut current = initial.clone();
        let mut applied = 0usize;
        let mut rolled_back = 0usize;
        let mut skipped = 0usize;
        for entry in &self.entries {
            let now = checksum(&current);
            if now != entry.before {
                return Err(Error::Audit(format!(
                    "第 {} 条 before={:016x}，当前工件 {:016x}",
                    entry.seq, entry.before, now
                )));
            }
            if !entry.applied {
                skipped += 1;
            } else {
                let journal = entry.patch.apply(&mut current)?;
                if entry.rolled_back {
                    journal.undo(&mut current);
                    rolled_back += 1;
                } else {
                    applied += 1;
                }
            }
            let after = checksum(&current);
            if after != entry.after {
                return Err(Error::Audit(format!(
                    "第 {} 条 after={:016x}，重放得到 {:016x}",
                    entry.seq, entry.after, after
                )));
            }
        }
        let report = ReplayReport {
            entries: self.entries.len(),
            applied,
            rolled_back,
            skipped,
            final_checksum: checksum(&current),
        };
        Ok((report, current))
    }

    /// 序列化为 JSONL（一行一条记录）。
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for entry in &self.entries {
            let value = json!({
                "v": LOG_VERSION,
                "seq": entry.seq,
                "name": entry.name,
                "before": hex(entry.before),
                "after": hex(entry.after),
                "applied": entry.applied,
                "rolled_back": entry.rolled_back,
                "cost": entry.cost,
                "patch": wire::patch_to_value(&entry.patch),
                "prev": hex(entry.prev),
                "chain": hex(entry.chain),
            });
            out.push_str(&value.to_string());
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
            let version = value
                .get("v")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::Audit(format!("第 {} 行缺少 v", index + 1)))?;
            if version != LOG_VERSION {
                return Err(Error::Audit(format!(
                    "第 {} 行版本 {version}（当前 {LOG_VERSION}）",
                    index + 1
                )));
            }
            let patch_value = value
                .get("patch")
                .ok_or_else(|| Error::Audit(format!("第 {} 行缺少 patch", index + 1)))?;
            let patch = wire::patch_from_value(patch_value)?;
            log.entries.push(LogEntry {
                seq: usize_field(&value, "seq")?,
                name: value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                before: hex_field(&value, "before")?,
                after: hex_field(&value, "after")?,
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
                prev: hex_field(&value, "prev")?,
                chain: hex_field(&value, "chain")?,
            });
        }
        Ok(log)
    }
}

fn chain_of(entry: &LogEntry) -> u64 {
    let canonical = json!({
        "v": LOG_VERSION,
        "seq": entry.seq,
        "name": entry.name,
        "before": hex(entry.before),
        "after": hex(entry.after),
        "applied": entry.applied,
        "rolled_back": entry.rolled_back,
        "cost": entry.cost,
        "patch": wire::patch_to_value(&entry.patch),
        "prev": hex(entry.prev),
    });
    fnv(canonical.to_string().as_bytes())
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn hex(value: u64) -> String {
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

fn usize_field(value: &Value, name: &str) -> Result<usize> {
    let number = value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Audit(format!("缺少整数字段 `{name}`")))?;
    usize::try_from(number).map_err(|_| Error::Audit(format!("`{name}` 超出 usize")))
}
