//! 程序 = 补丁 + 前置/后置行为条件。
//!
//! 这是“权重空间编程”的执行语义：程序要么**整体成功**（前置成立、补丁应用、
//! 后置成立），要么**原样回滚**。权重工件不会停在半改状态，因此可以对同一份
//! 权重反复试程序、批量审计。

use crate::edit::{Cost, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::{Behavior, Oracle};

/// 一个可执行、可回滚的权重编辑程序。
#[derive(Clone, Debug, PartialEq)]
pub struct Program {
    name: String,
    before: Option<Behavior>,
    patch: Patch,
    after: Option<Behavior>,
}

/// 一次执行的完整记录。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// 程序名。
    pub name: String,
    /// 补丁是否被应用过（前置不成立时为 `false`）。
    pub applied: bool,
    /// 后置不成立而回滚。
    pub rolled_back: bool,
    /// 前置条件是否成立。
    pub before_ok: bool,
    /// 应用后的行为是否满足后置条件。
    pub after_ok: bool,
    /// 执行前行为指纹。
    pub before_signature: u64,
    /// 补丁应用后、回滚前的行为指纹。
    pub changed_signature: u64,
    /// 最终行为指纹（回滚后等于 `before_signature`）。
    pub final_signature: u64,
    /// 声明的改动量。
    pub cost: Cost,
}

impl Program {
    /// 新建程序。
    #[must_use]
    pub fn new(name: impl Into<String>, patch: Patch) -> Self {
        Self {
            name: name.into(),
            before: None,
            patch,
            after: None,
        }
    }

    /// 要求执行前的行为必须等于 `behavior`。
    #[must_use]
    pub fn require_before(mut self, behavior: Behavior) -> Self {
        self.before = Some(behavior);
        self
    }

    /// 要求执行后的行为必须等于 `behavior`；不满足则回滚。
    #[must_use]
    pub fn require_after(mut self, behavior: Behavior) -> Self {
        self.after = Some(behavior);
        self
    }

    /// 程序名。
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 补丁。
    #[must_use]
    pub fn patch(&self) -> &Patch {
        &self.patch
    }

    /// 执行：前置检查 → 应用补丁 → 后置检查（失败回滚）。
    pub fn run(&self, net: &mut Net, oracle: &Oracle) -> Result<Report> {
        let before = Behavior::of(net, oracle)?;
        let before_signature = before.signature();
        let before_ok = self.before.as_ref().is_none_or(|want| want == &before);
        let cost = self.patch.cost(net);
        if !before_ok {
            return Ok(Report {
                name: self.name.clone(),
                applied: false,
                rolled_back: false,
                before_ok,
                after_ok: false,
                before_signature,
                changed_signature: before_signature,
                final_signature: before_signature,
                cost,
            });
        }

        let journal = self.patch.apply(net)?;
        let changed = Behavior::of(net, oracle)?;
        let changed_signature = changed.signature();
        let after_ok = self.after.as_ref().is_none_or(|want| want == &changed);
        let mut final_signature = changed_signature;
        if !after_ok {
            journal.undo(net);
            final_signature = Behavior::of(net, oracle)?.signature();
        }
        Ok(Report {
            name: self.name.clone(),
            applied: true,
            rolled_back: !after_ok,
            before_ok,
            after_ok,
            before_signature,
            changed_signature,
            final_signature,
            cost,
        })
    }
}
