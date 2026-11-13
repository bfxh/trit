//! TIES 的 trim 步骤（从 `task_merge.rs` 拆出，控制单文件行数）。
//!
//! 三值空间里一票的幅度 |Δ| 只有 0/1/2 三档：符号翻转、与 0 互换、没变。
//! 所以 trim 退化成“按档位裁剪”：先保符号翻转的票，再保 ±0 互换的票。

use crate::error::Result;
use crate::net::Net;
use crate::task::TaskVector;
use crate::trit::Trit;

use super::{MergeReport, merge_weighted, validate_weights};

/// TIES 的 trim 保留规则。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrimTop {
    /// 每个坐标保留 |Δ| 最大的 N 票。
    Count(usize),
    /// 每个坐标保留 |Δ| 最大的 ceil(ratio × 票数) 票（至少 1）。
    Fraction(f32),
}

/// TIES 的 trim 步骤：三值空间里 |Δ| 只有 0/1/2 三档，按幅度裁剪就是
/// “先保符号翻转、再保单档改动、最后保没变的票”。
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrimOptions {
    /// 每个坐标保留 |Δ| 最大的票；None 不按票数裁剪。
    pub top: Option<TrimTop>,
    /// 加权票差门槛：|Σ wᵢ·Δᵢ| < margin 的坐标整条丢弃（保留 base）。
    pub margin: f64,
}

impl TrimOptions {
    /// 每坐标保留多少票（available 是该坐标收到的票数）。
    #[must_use]
    pub fn keep(&self, available: usize) -> usize {
        match self.top {
            None => available,
            Some(TrimTop::Count(count)) => count.min(available),
            Some(TrimTop::Fraction(ratio)) => {
                let scaled = (f64::from(ratio) * available as f64).ceil() as usize;
                scaled.clamp(1, available)
            }
        }
    }

    /// 是否完全不裁剪。
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.top.is_none() && self.margin == 0.0
    }
}

/// 一票的 |Δ|：目标 trit 与基准值之差。
fn vote_magnitude(vote: &(Trit, f64), current: Trit) -> i8 {
    (vote.0.to_i8() - current.to_i8()).abs()
}

/// 裁剪一个坐标的票并返回加权票和；调用方再用 `trim.margin` 判是否丢弃。
pub(super) fn trim_and_sum(
    targets: &mut Vec<(Trit, f64)>,
    current: Trit,
    trim: &TrimOptions,
) -> f64 {
    if trim.top.is_some() {
        targets.sort_by(|a, b| {
            vote_magnitude(b, current)
                .cmp(&vote_magnitude(a, current))
                .then_with(|| b.1.total_cmp(&a.1))
        });
        let keep = trim.keep(targets.len());
        targets.truncate(keep);
    }
    targets
        .iter()
        .map(|(target, weight)| f64::from(target.to_i8()) * weight)
        .sum()
}

/// TIES 式合并：先逐坐标裁剪票（[`TrimOptions`]），再投票。
///
/// `TrimOptions::default()` 与 [`super::merge_tasks_weighted_report`] 逐位一致。
pub fn merge_tasks_weighted_trim_report(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    trim: &TrimOptions,
) -> Result<(Net, MergeReport)> {
    validate_weights(tasks.len(), weights)?;
    merge_weighted(base, tasks, weights, trim)
}

/// TIES 式合并（不带报告）。
pub fn merge_tasks_weighted_trim(
    base: &Net,
    tasks: &[TaskVector],
    weights: &[f32],
    trim: &TrimOptions,
) -> Result<Net> {
    Ok(merge_tasks_weighted_trim_report(base, tasks, weights, trim)?.0)
}
