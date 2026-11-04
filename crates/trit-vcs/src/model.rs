//! 提交模型：一条提交自带会话身份、认领、证据与标签。
//!
//! 与 Git 的差别：Git 的提交只有 author/committer 字符串；这里把"谁（宿主:会话id）、
//! 在哪个认领下、带了什么证据、属于哪些技术标签"一起钉进对象内容——审计与查询都不需要再翻别处。
//! 时间用混合逻辑时钟（HLC）：墙上时钟乱序也能给全序。

use serde::{Deserialize, Serialize};

use crate::hash::Hash;

/// 混合逻辑时钟：`(wall_ms, counter)`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
}

impl Hlc {
    pub fn zero() -> Self {
        Self {
            wall_ms: 0,
            counter: 0,
        }
    }

    /// 观测到 `observed` 之后推进一次：时间更晚就重置计数，否则计数 +1。
    pub fn tick(self, observed: u64) -> Self {
        if observed > self.wall_ms {
            Self {
                wall_ms: observed,
                counter: 0,
            }
        } else {
            Self {
                wall_ms: self.wall_ms,
                counter: self.counter.saturating_add(1),
            }
        }
    }
}

/// 提交对象的内容（存进对象库，id = 内容哈希）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Commit {
    pub schema: u32,
    pub tree: Hash,
    pub parents: Vec<Hash>,
    /// `宿主:会话id`
    pub session: String,
    /// 覆盖本次改动的认领 id（可以多条：一个会话同时守几个范围）
    pub claims: Vec<String>,
    /// 证据：PR 号、读数、门禁结论等自由文本（索引与审计直接读这里）
    pub evidence: Vec<String>,
    pub tags: Vec<String>,
    pub message: String,
    pub hlc: Hlc,
    /// 绑定了本次树的测试见证（对象库里的摘要）；老提交没有这个字段。
    #[serde(default)]
    pub witness: Option<Hash>,
}

pub const COMMIT_SCHEMA: u32 = 1;

impl Commit {
    pub fn new(
        tree: Hash,
        session: impl Into<String>,
        message: impl Into<String>,
        hlc: Hlc,
    ) -> Self {
        Self {
            schema: COMMIT_SCHEMA,
            tree,
            parents: Vec::new(),
            session: session.into(),
            claims: Vec::new(),
            evidence: Vec::new(),
            tags: Vec::new(),
            message: message.into(),
            hlc,
            witness: None,
        }
    }

    pub fn with_witness(mut self, witness: Hash) -> Self {
        self.witness = Some(witness);
        self
    }

    pub fn with_parent(mut self, parent: Hash) -> Self {
        self.parents.push(parent);
        self
    }

    pub fn with_claim(mut self, claim: impl Into<String>) -> Self {
        self.claims.push(claim.into());
        self
    }

    pub fn with_evidence(mut self, evidence: impl Into<String>) -> Self {
        self.evidence.push(evidence.into());
        self
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    pub fn encode(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    pub fn id(&self) -> Result<Hash, serde_json::Error> {
        Ok(Hash::of(&self.encode()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hlc_is_monotonic_under_skew() {
        let first = Hlc::zero().tick(100);
        assert_eq!(
            first,
            Hlc {
                wall_ms: 100,
                counter: 0
            }
        );
        let backwards = first.tick(90); // 墙上时钟倒退
        assert_eq!(
            backwards,
            Hlc {
                wall_ms: 100,
                counter: 1
            }
        );
        assert!(backwards > first, "HLC 必须仍然单调");
        let forwards = backwards.tick(200);
        assert_eq!(
            forwards,
            Hlc {
                wall_ms: 200,
                counter: 0
            }
        );
    }

    #[test]
    fn commit_id_is_content_hash() {
        let commit = Commit::new(Hash::of(b"tree"), "qoder:s1", "msg", Hlc::zero().tick(1));
        let again = Commit::new(Hash::of(b"tree"), "qoder:s1", "msg", Hlc::zero().tick(1));
        assert_eq!(commit.id().unwrap(), again.id().unwrap());
        let different = commit.clone().with_tag("gpu");
        assert_ne!(
            commit.id().unwrap(),
            different.id().unwrap(),
            "改一个字段 id 必须变"
        );
    }

    #[test]
    fn commit_roundtrip_keeps_evidence() {
        let commit = Commit::new(Hash::of(b"tree"), "zcode:zoder", "x", Hlc::zero().tick(5))
            .with_claim("c1")
            .with_evidence("PR #246")
            .with_tag("gpu");
        let decoded = Commit::decode(&commit.encode().unwrap()).unwrap();
        assert_eq!(decoded, commit);
        assert_eq!(decoded.evidence, vec!["PR #246".to_string()]);
    }

    #[test]
    fn old_commits_without_witness_still_decode() {
        let commit = Commit::new(Hash::of(b"tree"), "qoder:s1", "old", Hlc::zero().tick(1));
        let mut value: serde_json::Value =
            serde_json::from_slice(&commit.encode().unwrap()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("witness")
            .expect("老提交里没有 witness 字段");
        let decoded = Commit::decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(decoded.witness.is_none());
        assert_eq!(decoded.message, "old");
    }
}
