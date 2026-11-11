//! 扫描发现与规则结论。
//!
//! 扫描结论复用 [`trit_core::Ternary`]：`True` = 通过，`False` = 失败，
//! `Unknown` = 证据不足（不能当通过）。聚合使用 Kleene 的格序合取。

use crate::hash::Fnv64;
use crate::snapshot::EntityId;
use trit_core::Ternary;

/// 发现的严重度。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Severity {
    /// 低危/加固建议。
    Low = 0,
    /// 中危。
    Medium = 1,
    /// 高危。
    High = 2,
    /// 严重，优先处理。
    Critical = 3,
}

/// 单条扫描发现。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// 产生该发现的规则 ID。
    pub rule_id: &'static str,
    /// 严重度。
    pub severity: Severity,
    /// 三值结论：`False` 失败、`Unknown` 无法判定、`True` 通过。
    pub trit: Ternary,
    /// 关联实体（如果有）。
    pub target: Option<EntityId>,
    /// 面向人的简述。
    pub message: String,
    /// 结构化证据/触发路径。
    pub evidence: String,
}

impl Finding {
    /// 构造一条失败发现。
    pub fn fail(
        rule_id: &'static str,
        severity: Severity,
        message: impl Into<String>,
        evidence: impl Into<String>,
    ) -> Self {
        Self {
            rule_id,
            severity,
            trit: Ternary::False,
            target: None,
            message: message.into(),
            evidence: evidence.into(),
        }
    }

    /// 构造一条“证据不足、不能判定”的发现。
    pub fn unknown(
        rule_id: &'static str,
        severity: Severity,
        message: impl Into<String>,
        evidence: impl Into<String>,
    ) -> Self {
        Self {
            rule_id,
            severity,
            trit: Ternary::Unknown,
            target: None,
            message: message.into(),
            evidence: evidence.into(),
        }
    }

    /// 绑定目标实体。
    pub fn with_target(mut self, target: EntityId) -> Self {
        self.target = Some(target);
        self
    }
}

/// 单条规则的执行结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleOutcome {
    /// 规则 ID。
    pub rule_id: &'static str,
    /// 三值结论。
    pub trit: Ternary,
    /// 该规则产生的发现；通过时为空。
    pub findings: Vec<Finding>,
}

impl RuleOutcome {
    /// 规则通过。
    pub fn pass(rule_id: &'static str) -> Self {
        Self {
            rule_id,
            trit: Ternary::True,
            findings: Vec::new(),
        }
    }

    /// 规则失败。
    pub fn fail(rule_id: &'static str, findings: Vec<Finding>) -> Self {
        debug_assert!(!findings.is_empty());
        Self {
            rule_id,
            trit: Ternary::False,
            findings,
        }
    }

    /// 规则无法判定。
    pub fn unknown(rule_id: &'static str, findings: Vec<Finding>) -> Self {
        debug_assert!(!findings.is_empty());
        Self {
            rule_id,
            trit: Ternary::Unknown,
            findings,
        }
    }
}

/// 一次扫描的完整报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanReport {
    /// 被扫描快照的规范化哈希。
    pub snapshot_hash: u64,
    /// 各规则的结果。
    pub outcomes: Vec<RuleOutcome>,
}

impl ScanReport {
    /// 所有规则的 Kleene 合取：任一失败即失败；否则任一未知即未知；否则通过。
    pub fn trit(&self) -> Ternary {
        self.outcomes
            .iter()
            .fold(Ternary::True, |acc, outcome| acc & outcome.trit)
    }

    /// 所有发现。
    pub fn findings(&self) -> impl Iterator<Item = &Finding> {
        self.outcomes
            .iter()
            .flat_map(|outcome| outcome.findings.iter())
    }

    /// 报告签名，用于确定性和恢复前后的基线对比。
    pub fn signature(&self) -> u64 {
        let mut hash = Fnv64::new();
        hash.write_u64(self.snapshot_hash);
        hash.write_u64(self.outcomes.len() as u64);
        for outcome in &self.outcomes {
            hash.write_str(outcome.rule_id);
            hash.write_u64(outcome.trit.index() as u64);
            hash.write_u64(outcome.findings.len() as u64);
            for finding in &outcome.findings {
                hash.write_str(finding.rule_id);
                hash.write_u64(finding.severity as u64);
                hash.write_u64(finding.trit.index() as u64);
                match finding.target {
                    Some(target) => {
                        hash.write(&[1]);
                        hash.write_u64(target);
                    }
                    None => hash.write(&[0]),
                }
                hash.write_str(&finding.message);
                hash.write_str(&finding.evidence);
            }
        }
        hash.finish()
    }
}
