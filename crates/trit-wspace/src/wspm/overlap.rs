//! WSPM 条目 blob 区间不相交校验（#631）。
//!
//! 按 `offset` 排序后相邻半开区间不得相交；首尾相接 `[a,b)` 与 `[b,c)` 允许。
//! 相交时复用既有 [`Error::Wire`] 变体，文案点名冲突的两个张量。

use super::ModelEntry;
use crate::error::{Error, Result};

/// 校验全部条目的 blob 区间：不越出 payload 尾、互不相交。
pub(super) fn validate_regions(entries: &[ModelEntry], payload_end: usize) -> Result<()> {
    let mut regions: Vec<(u64, u64, &str)> = Vec::with_capacity(entries.len());
    for entry in entries {
        let end = match entry.offset.checked_add(entry.len) {
            Some(end) if end <= payload_end as u64 => end,
            _ => {
                return Err(Error::Wire(format!(
                    "`{}` blob 越界（offset {} len {}）",
                    entry.name, entry.offset, entry.len
                )));
            }
        };
        regions.push((entry.offset, end, entry.name.as_str()));
    }
    regions.sort_by_key(|(start, _, _)| *start);
    for pair in regions.windows(2) {
        let (_, first_end, first) = pair[0];
        let (second_start, _, second) = pair[1];
        if second_start < first_end {
            return Err(Error::Wire(format!(
                "blob 区间相交：`{first}` 与 `{second}` 的 [offset, offset+len) 重叠"
            )));
        }
    }
    Ok(())
}
