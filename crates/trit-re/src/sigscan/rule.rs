//! 规则与报告类型（从 `sigscan.rs` 拆出守预算）：规则装配、`private`
//! 报告过滤与逐规则裁决。

use super::{Condition, Pattern};
use trit_core::value::Ternary;

/// 一条签名规则：名字 + 字符串表 + 条件。
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub(super) patterns: Vec<(String, Pattern)>,
    pub(super) condition: Condition,
    /// `private` 串下标：匹配与条件引用照常，但**不出现在命中报告**。
    pub(super) private: Vec<usize>,
}

impl Rule {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            patterns: Vec::new(),
            condition: Condition::All,
            private: Vec::new(),
        }
    }

    pub fn push(mut self, id: &str, pattern: Pattern) -> Self {
        self.patterns.push((id.to_string(), pattern));
        self
    }

    pub fn condition(mut self, condition: Condition) -> Self {
        self.condition = condition;
        self
    }

    /// 供同 crate 的解析器装配（字段保持私有，外部仍走 new/push/condition）。
    pub(crate) fn from_parts(
        name: String,
        patterns: Vec<(String, Pattern)>,
        condition: Condition,
        private: Vec<usize>,
    ) -> Self {
        Self {
            name,
            patterns,
            condition,
            private,
        }
    }

    /// 串表（`$id` 去掉 `$` 后的 id + 模式）——解析器据此把 `$id` 引用
    /// 解析成下标。
    pub fn patterns(&self) -> &[(String, Pattern)] {
        &self.patterns
    }

    pub fn condition_ref(&self) -> &Condition {
        &self.condition
    }
}

/// 一条规则的裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleVerdict {
    pub rule: String,
    pub value: Ternary,
    /// 命中的模式（`模式id, 命中偏移` 列表；每模式最多 [`super::MAX_HITS`] 个）。
    pub hits: Vec<(String, u64)>,
}

/// 一次扫描的报告：逐规则裁决 + 整体（Kleene OR 报警语义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigReport {
    pub rules: Vec<RuleVerdict>,
    pub value: Ternary,
}

impl Rule {
    /// 对外报告用的命中：`private` 串隐藏（条件求值仍用全量 occurrences）。
    pub(crate) fn report_hits(&self, occurrences: &[(String, Vec<u64>)]) -> Vec<(String, u64)> {
        occurrences
            .iter()
            .enumerate()
            .filter(|(i, _)| !self.private.contains(i))
            .flat_map(|(_, (id, offs))| offs.iter().map(move |o| (id.clone(), *o)))
            .collect()
    }
}
