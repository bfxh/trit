//! 分片清单：文件级索引，防止丢片 / 错序 / 内容替换。
//!
//! 清单记录每个分片的序号、起止、链头与**文本摘要**；`shard` 命令写出
//! `manifest.json`，`audit-shards` 默认读取它来决定读哪些文件、按什么顺序读，
//! 并在流式读取时逐片核对摘要。没有清单时退回按文件名排序的旧行为。

use serde_json::{Value, json};

use crate::audit::{fnv, hex};
use crate::error::{Error, Result};
use crate::shard::ShardedLog;

/// 清单版本。
pub const MANIFEST_VERSION: u64 = 1;

/// 单个分片的索引项。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestShard {
    /// 分片序号。
    pub shard: usize,
    /// 文件名（CLI 写盘时填写；MCP 返回的清单为 `None`）。
    pub file: Option<String>,
    /// 本片第一条记录的全局序号。
    pub first_seq: usize,
    /// 记录条数。
    pub count: usize,
    /// 本片链头。
    pub head: u64,
    /// 分片文本的 FNV-1a 摘要。
    pub digest: u64,
}

/// 分片清单。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardManifest {
    /// 版本。
    pub version: u64,
    /// 记录总数。
    pub entries: usize,
    /// 整条链的链头。
    pub head: u64,
    /// 分片索引（按序）。
    pub shards: Vec<ManifestShard>,
}

/// 分片文本的摘要。
#[must_use]
pub fn text_digest(text: &str) -> u64 {
    fnv(text.as_bytes())
}

impl ShardManifest {
    /// 从分片日志生成清单（未填文件名）。
    #[must_use]
    pub fn of(log: &ShardedLog) -> Self {
        let texts = log.to_texts();
        let shards = log
            .shards()
            .iter()
            .zip(texts.iter())
            .map(|(shard, text)| ManifestShard {
                shard: shard.header().shard,
                file: None,
                first_seq: shard.header().first_seq,
                count: shard.header().count,
                head: shard.header().head,
                digest: text_digest(text),
            })
            .collect();
        Self {
            version: MANIFEST_VERSION,
            entries: log.len(),
            head: log.head(),
            shards,
        }
    }

    /// 序列化为 JSON 值。
    #[must_use]
    pub fn to_value(&self) -> Value {
        json!({
            "version": self.version,
            "entries": self.entries,
            "head": hex(self.head),
            "shards": self.shards.iter().map(|shard| json!({
                "shard": shard.shard,
                "file": shard.file,
                "first_seq": shard.first_seq,
                "count": shard.count,
                "head": hex(shard.head),
                "digest": hex(shard.digest),
            })).collect::<Vec<_>>(),
        })
    }

    /// 解析 JSON 值并校验形状。
    pub fn from_value(value: &Value) -> Result<Self> {
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Audit("manifest 缺少 version".into()))?;
        if version != MANIFEST_VERSION {
            return Err(Error::Audit(format!(
                "manifest 版本 {version}（当前 {MANIFEST_VERSION}）"
            )));
        }
        let entries = usize_field(value, "entries")?;
        let head = hex_field(value, "head", "manifest")?;
        let raw = value
            .get("shards")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Audit("manifest 缺少 shards 数组".into()))?;
        let mut shards = Vec::with_capacity(raw.len());
        for (index, item) in raw.iter().enumerate() {
            let shard = usize_field(item, "shard")?;
            if shard != index {
                return Err(Error::Audit(format!(
                    "manifest 第 {index} 项 shard={shard}（应连续递增）"
                )));
            }
            shards.push(ManifestShard {
                shard,
                file: item.get("file").and_then(Value::as_str).map(str::to_string),
                first_seq: usize_field(item, "first_seq")?,
                count: usize_field(item, "count")?,
                head: hex_field(item, "head", &format!("manifest 第 {index} 项"))?,
                digest: hex_field(item, "digest", &format!("manifest 第 {index} 项"))?,
            });
        }
        Ok(Self {
            version,
            entries,
            head,
            shards,
        })
    }

    /// 按清单核对分片文本：数量、顺序、条数、链头与摘要。
    pub fn verify_texts(&self, texts: &[String]) -> Result<()> {
        if self.shards.len() != texts.len() {
            return Err(Error::Audit(format!(
                "manifest 声明 {} 片，实际 {} 片",
                self.shards.len(),
                texts.len()
            )));
        }
        let mut total = 0usize;
        for (index, (item, text)) in self.shards.iter().zip(texts).enumerate() {
            let digest = text_digest(text);
            if digest != item.digest {
                return Err(Error::Audit(format!(
                    "第 {index} 片摘要={digest:016x}，manifest 记录 {:016x}",
                    item.digest
                )));
            }
            total += item.count;
        }
        if total != self.entries {
            return Err(Error::Audit(format!(
                "manifest 条数={}，各片合计 {total}",
                self.entries
            )));
        }
        if let Some(last) = self.shards.last()
            && last.head != self.head
        {
            return Err(Error::Audit(format!(
                "manifest head={:016x}，最后一片 {:016x}",
                self.head, last.head
            )));
        }
        Ok(())
    }
}

fn usize_field(value: &Value, name: &str) -> Result<usize> {
    let number = value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::Audit(format!("manifest 缺少整数字段 `{name}`")))?;
    usize::try_from(number).map_err(|_| Error::Audit(format!("manifest `{name}` 超出 usize")))
}

fn hex_field(value: &Value, name: &str, where_: &str) -> Result<u64> {
    let text = value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Audit(format!("{where_} 缺少 `{name}`")))?;
    u64::from_str_radix(text, 16)
        .map_err(|_| Error::Audit(format!("{where_} 的 `{name}` 不是十六进制：{text}")))
}
