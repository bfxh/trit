//! 张量数据区全局区间校验（R41-01）。
//!
//! 信息段收集完成、数据段起点对齐后，统一构造各张量区间并复核：
//! - 每张量区间 `[start, end)` 不得伸入对齐前的信息段（`start < data_start` 即拒）；
//! - 按 `start` 排序后相邻区间不得相交；首尾相接 `[a, b)` 与 `[b, c)` 允许；
//! - 相交时错误点名冲突的两个张量及各自区间。
//!
//! 单张量尾部越出文件尾的检查保留在 [`crate::read`] 解码循环；本模块只负责区间
//! 对账与解码所需偏移量，不做字节解码。

use crate::{Error, Info};

/// 解码准备好的张量区间：信息段已对账，区间两两不相交。
pub(crate) struct Prepared {
    /// 区间起点（绝对文件偏移）。
    pub start: usize,
    /// trit 打包字节数。
    pub packed: usize,
    /// trit 总数。
    pub trit_len: usize,
    /// 区间终点（不含）。
    pub end: usize,
}

/// 构造各张量区间，复核不得伸入信息段且两两不相交；返回解码用准备项。
pub(crate) fn prepare(data_start: usize, infos: &[Info]) -> Result<Vec<Prepared>, Error> {
    let mut prepared: Vec<Prepared> = Vec::with_capacity(infos.len());
    let mut regions: Vec<(usize, usize, &str)> = Vec::with_capacity(infos.len());
    for info in infos {
        let start = data_start
            .checked_add(info.offset)
            .ok_or_else(|| Error::Tensor(info.name.clone()))?;
        if start < data_start {
            return Err(Error::Tensor(format!("{} 区间伸入信息段", info.name)));
        }
        let trit_len = info.rows.saturating_mul(info.cols);
        let packed = trit_len.div_ceil(5);
        let bias_bytes = info.rows.div_ceil(5);
        let end = start
            .checked_add(packed)
            .and_then(|value| value.checked_add(info.rows.saturating_mul(4)))
            .and_then(|value| value.checked_add(bias_bytes))
            .ok_or_else(|| Error::Tensor(info.name.clone()))?;
        regions.push((start, end, info.name.as_str()));
        prepared.push(Prepared {
            start,
            packed,
            trit_len,
            end,
        });
    }
    let mut ordered = regions;
    ordered.sort_unstable_by_key(|(start, _, _)| *start);
    for pair in ordered.windows(2) {
        let (first_start, first_end, first) = pair[0];
        let (second_start, second_end, second) = pair[1];
        if second_start < first_end {
            return Err(Error::Tensor(format!(
                "{} 与 {} 数据区重叠：[{}, {}) 与 [{}, {})",
                first, second, first_start, first_end, second_start, second_end
            )));
        }
    }
    Ok(prepared)
}
