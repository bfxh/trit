//! 审计日志幂等：同一条编辑重复记录时只保留一条。
//!
//! 幂等键 = `name + before + after + applied + rolled_back + patch`，并且只与**最后一条**
//! 比较：中间插入过别的编辑就说明工件状态已经前进，此时再记一条是合法的历史。
//! 这样「重复应用同一补丁」不会把日志撑大，而「改回原样再改一次」仍会留下两条。

use crate::edit::Patch;

use super::AuditLog;

impl AuditLog {
    /// 本次记录是否与最后一条完全相同（幂等键）。
    pub(crate) fn is_duplicate(
        &self,
        name: &str,
        before: u64,
        after: u64,
        patch: &Patch,
        applied: bool,
        rolled_back: bool,
    ) -> bool {
        self.entries().last().is_some_and(|entry| {
            entry.name == name
                && entry.before == before
                && entry.after == after
                && entry.applied == applied
                && entry.rolled_back == rolled_back
                && entry.patch == *patch
        })
    }
}
