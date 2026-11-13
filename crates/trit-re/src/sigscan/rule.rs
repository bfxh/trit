//! 规则类型（从 `sigscan.rs` 拆出守预算）：名字、串表、条件、global/private。

use super::{Condition, Pattern};

/// 一条签名规则：名字 + 字符串表 + 条件。
#[derive(Debug, Clone)]
pub struct Rule {
    pub name: String,
    pub(super) patterns: Vec<(String, Pattern)>,
    pub(super) condition: Condition,
    /// `private` 串下标：匹配与条件引用照常，但不出现在命中报告。
    pub(super) private: Vec<usize>,
    pub global: bool,
    pub private_rule: bool,
}

impl Rule {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            patterns: Vec::new(),
            condition: Condition::All,
            private: Vec::new(),
            global: false,
            private_rule: false,
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
    ) -> Self {
        Self {
            name,
            patterns,
            condition,
            private: Vec::new(),
            global: false,
            private_rule: false,
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

impl Rule {
    /// 设置 global / private 规则标志（解析器用）。
    pub fn with_flags(mut self, global: bool, private_rule: bool) -> Self {
        self.global = global;
        self.private_rule = private_rule;
        self
    }

    /// 设置 private 串下标（解析器用）。
    pub fn with_private(mut self, private: Vec<usize>) -> Self {
        self.private = private;
        self
    }
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
