//! WSP2 外部块索引的预算校验（从 `index.rs` 拆出，守单文件行数预算）。
//!
//! 索引是独立 JSON，不能只看 offset 顺序：`rows` / `rows*cols` 直接决定
//! `read_rows_io` 的 `Vec` 容量。任何合法层的 `rows*cols` 都不可能超过
//! `source_bytes*4`（trits 每 4 元素 1 字节），这是安全的必要上界。

use super::index::BlockIndex;
use crate::error::{Error, Result};

pub(super) fn check(index: &BlockIndex) -> Result<()> {
    if index.source_bytes < 8 {
        return Err(Error::Wire(format!(
            "索引 source_bytes {} 小于 8（不可能是 WSP2）",
            index.source_bytes
        )));
    }
    let max_elems = index.source_bytes.saturating_mul(4);
    for info in &index.layers {
        if info.rows > index.source_bytes {
            return Err(Error::Wire(format!(
                "索引层 {} 的 rows {} 超过源文件字节数 {}（预算拒绝）",
                info.index, info.rows, index.source_bytes
            )));
        }
        let elems = info
            .rows
            .checked_mul(info.cols)
            .ok_or_else(|| Error::Wire(format!("索引层 {} 的 rows*cols 溢出", info.index)))?;
        if elems > max_elems {
            return Err(Error::Wire(format!(
                "索引层 {} 声明 {elems} 个元素，超过源文件预算 {max_elems}（预算拒绝）",
                info.index
            )));
        }
        for (name, offset) in [
            ("layer_offset", info.layer_offset),
            ("scales_offset", info.scales_offset),
            ("trits_offset", info.trits_offset),
            ("bias_offset", info.bias_offset),
            ("end_offset", info.end_offset),
        ] {
            if offset > index.source_bytes {
                return Err(Error::Wire(format!(
                    "索引层 {} 的 {name} {offset} 超过源文件字节数 {}（预算拒绝）",
                    info.index, index.source_bytes
                )));
            }
        }
        if let Some(offset) = info.bias_f32_offset
            && offset > index.source_bytes
        {
            return Err(Error::Wire(format!(
                "索引层 {} 的 bias_f32_offset {offset} 超过源文件字节数 {}（预算拒绝）",
                info.index, index.source_bytes
            )));
        }
        for block in &info.row_blocks {
            let end = block
                .trits_offset
                .checked_add(block.trits_len)
                .ok_or_else(|| Error::Wire("索引行块 trits 区间溢出".into()))?;
            if end > index.source_bytes {
                return Err(Error::Wire(format!(
                    "索引层 {} 的行块 {}..{} trits 区间结束 {end} 超过源文件字节数 {}（预算拒绝）",
                    info.index, block.row_start, block.row_end, index.source_bytes
                )));
            }
        }
    }
    Ok(())
}
