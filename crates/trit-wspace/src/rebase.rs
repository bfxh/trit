//! 三方补丁重放：把针对 `base` 写的补丁落到已经前进到 `target` 的工件上。
//!
//! 每条编辑先比较 `base` 与 `target` 在同一坐标上的状态：
//!
//! - 两边相同 ⇒ 这条编辑可以应用；
//! - `target` 已经是补丁想要的值（行操作则是已经做过）⇒ 记为已应用并跳过；
//! - 两边不同且不是已应用 ⇒ 冲突，绝不静默覆盖。
//!
//! 冲突按**整体**判定：没有开 `skip_conflicts` 时 `rejected=true`，调用方不得应用
//! 返回的 [`RebaseReport::clean`]；开了才只落干净的那部分，冲突仍逐条报告。

use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;

mod classify;
use classify::classify;

/// 一次重放的分类结果（不改任何工件）。
#[derive(Clone, Debug)]
pub struct RebaseReport {
    /// 与 `target` 不冲突、需要应用的编辑（保持原顺序）。
    pub clean: Patch,
    /// 冲突列表（按补丁顺序）。
    pub conflicts: Vec<Conflict>,
    /// `target` 上已经是目标值、无需再应用的编辑条数。
    pub already_applied: usize,
    /// 有冲突且未允许跳过；此时 `clean` 不得应用。
    pub rejected: bool,
}

/// 一条冲突：补丁想把 `base` 上的值改成目标值，但 `target` 已经不一样了。
#[derive(Clone, Debug, PartialEq)]
pub struct Conflict {
    /// 冲突的编辑。
    pub edit: Edit,
    /// `base` 上被这条编辑碰到的状态（点编辑是三值字符，scale 是数值，行操作是行指纹）。
    pub base: String,
    /// `target` 上同一位置的状态（格式同 `base`）。
    pub target: String,
}

enum Verdict {
    Clean,
    AlreadyApplied,
    Conflict { base: String, target: String },
}

/// 三方重放分类。`skip_conflicts=false` 时只要有一条冲突就 `rejected=true`。
///
/// `base` 是补丁写作时的工件，`target` 是现在要落到的工件；两者必须同架构。
pub fn rebase(
    base: &Net,
    target: &Net,
    patch: &Patch,
    skip_conflicts: bool,
) -> Result<RebaseReport> {
    if base.arch() != target.arch() {
        return Err(Error::ArchMismatch);
    }
    patch.validate(base)?;
    let mut clean = Vec::new();
    let mut conflicts = Vec::new();
    let mut already_applied = 0;
    for edit in patch.edits() {
        match classify(base, target, edit)? {
            Verdict::Clean => clean.push(edit.clone()),
            Verdict::AlreadyApplied => already_applied += 1,
            Verdict::Conflict { base, target } => conflicts.push(Conflict {
                edit: edit.clone(),
                base,
                target,
            }),
        }
    }
    let rejected = !conflicts.is_empty() && !skip_conflicts;
    Ok(RebaseReport {
        clean: Patch::new(clean),
        conflicts,
        already_applied,
        rejected,
    })
}
