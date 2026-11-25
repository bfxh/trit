//! WSPM 条目 blob 区间两两不相交校验（issue #631）。
//!
//! 修前解包时每个条目只判自身 `offset+len <= payload_end`，从不比较任意
//! 两个条目区间是否相交：B 的 `offset/len` 指向 A 的 blob 时，校验和按
//! 重叠后字节重算仍合法，`unpack` 退出 0，B 静默读出 A 的权重。
//!
//! 这里把全部条目区间按 `offset` 排序后逐对相邻比对；`prev_end <= next_start`
//! 放行（恰好相接合法），相交时报错并点名冲突的两个条目。

use super::entry::ModelEntry;
use crate::error::{Error, Result};

/// 校验所有条目 blob 区间互不相交，且不越出载荷段。
pub(super) fn check(entries: &[ModelEntry], payload_end: u64) -> Result<()> {
    let mut intervals: Vec<(u64, u64, &ModelEntry)> = entries
        .iter()
        .map(|entry| {
            let end = entry
                .offset
                .checked_add(entry.len)
                .ok_or_else(|| Error::Wire(format!("`{}` 区间溢出", entry.name)))?;
            if end > payload_end {
                return Err(Error::Wire(format!(
                    "`{}` blob 越界（offset {} len {}）",
                    entry.name, entry.offset, entry.len
                )));
            }
            Ok((entry.offset, end, entry))
        })
        .collect::<Result<Vec<_>>>()?;
    intervals.sort_by_key(|(start, _, _)| *start);

    for pair in intervals.windows(2) {
        let (_, prev_end, prev) = &pair[0];
        let (next_start, next_end, next) = &pair[1];
        if prev_end > next_start {
            return Err(Error::Wire(format!(
                "条目 `{}` 区间 {}..{} 与条目 `{}` 区间 {}..{} 重叠",
                prev.name, prev.offset, prev_end, next.name, next.offset, next_end
            )));
        }
    }
    Ok(())
}
