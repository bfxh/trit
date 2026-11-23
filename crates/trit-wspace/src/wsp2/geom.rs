//! WSP2 块索引的区间几何校验（issue #630）。
//!
//! 索引 JSON 是外部输入：偏移表被篡改后，只校验偏移单调顺序挡不住区间
//! 漂移，默认读路径（不携带 `--verify`）会把相邻区间的字节当成本层的
//! scales / bias 解码。这里按写侧（`write.rs` / `BlockIndex::scan`）的字节
//! 公式重算每层各段 `[offset, offset+len)`，校验：
//!
//! * 各段长度等式成立（scales / bias_f32 / trits / bias 字节数与 `rows/cols`
//!   / flags 对账）；
//! * 层内各段按序相邻、互不相交；
//! * 层终点不越 `source_bytes`；层间区间不重叠（恰好相接放行）；
//! * 行块表连续覆盖 `0..rows`、互不重叠，且不越出本层 trits 区。

use super::index::{BlockIndex, LayerIndex};
use crate::error::{Error, Result};

/// 对整份索引跑区间几何校验。
pub(super) fn check(index: &BlockIndex) -> Result<()> {
    for info in &index.layers {
        check_layer(info, index.source_bytes)?;
    }
    for pair in index.layers.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        if prev.end_offset > next.layer_offset {
            return Err(Error::Wire(format!(
                "索引层 {} 区间 {}..{} 与层 {} 起点 {} 重叠",
                prev.index, prev.layer_offset, prev.end_offset, next.index, next.layer_offset
            )));
        }
    }
    Ok(())
}

fn check_layer(info: &LayerIndex, source_bytes: usize) -> Result<()> {
    let rows = info.rows;
    let scales_len = rows.checked_mul(4).ok_or_else(|| overflow("scales"))?;
    let biasf_len = rows.checked_mul(4).ok_or_else(|| overflow("bias_f32"))?;
    let trit_elems = rows
        .checked_mul(info.cols)
        .ok_or_else(|| overflow("trits"))?;
    let trit_bytes = trit_elems.div_ceil(4);
    let bias_bytes = rows.div_ceil(4);

    let scales_end = add(info.scales_offset, scales_len)?;
    let after_scales = info.bias_f32_offset.unwrap_or(info.trits_offset);
    if scales_end != after_scales {
        return Err(Error::Wire(format!(
            "索引层 {} scales 区间 {}..{} 与下一段起点 {after_scales} 不符（rows={rows} 应长 {scales_len} 字节）",
            info.index, info.scales_offset, scales_end
        )));
    }
    if let Some(bias_f32_offset) = info.bias_f32_offset {
        let biasf_end = add(bias_f32_offset, biasf_len)?;
        if biasf_end != info.trits_offset {
            return Err(Error::Wire(format!(
                "索引层 {} bias_f32 区间 {}..{} 与 trits 起点 {} 不符（rows={rows} 应长 {biasf_len} 字节）",
                info.index, bias_f32_offset, biasf_end, info.trits_offset
            )));
        }
    }
    let trits_end = add(info.trits_offset, trit_bytes)?;
    if trits_end != info.bias_offset {
        return Err(Error::Wire(format!(
            "索引层 {} trits 区间 {}..{} 与 bias 起点 {} 不符（{} 个元素应长 {trit_bytes} 字节）",
            info.index, info.trits_offset, trits_end, info.bias_offset, trit_elems
        )));
    }
    let bias_end = add(info.bias_offset, bias_bytes)?;
    if bias_end != info.end_offset {
        return Err(Error::Wire(format!(
            "索引层 {} bias 区间 {}..{} 与层终点 {} 不符（rows={rows} 应长 {bias_bytes} 字节）",
            info.index, info.bias_offset, bias_end, info.end_offset
        )));
    }
    if info.end_offset > source_bytes {
        return Err(Error::Wire(format!(
            "索引层 {} 终点 {} 超出源文件 {source_bytes} 字节",
            info.index, info.end_offset
        )));
    }

    let mut expected_start = 0usize;
    for block in &info.row_blocks {
        if block.row_start != expected_start || block.row_end > rows {
            return Err(Error::Wire(format!(
                "索引层 {} 行块 {}..{} 未连续覆盖 0..{rows}",
                info.index, block.row_start, block.row_end
            )));
        }
        expected_start = block.row_end;
        let block_end = add(block.trits_offset, block.trits_len)?;
        if block.trits_offset < info.trits_offset || block_end > trits_end {
            return Err(Error::Wire(format!(
                "索引层 {} 行块 {}..{} trits 区间 {}..{} 越出本层 trits 区 {}..{}",
                info.index,
                block.row_start,
                block.row_end,
                block.trits_offset,
                block_end,
                info.trits_offset,
                trits_end
            )));
        }
    }
    if expected_start != rows {
        return Err(Error::Wire(format!(
            "索引层 {} 行块覆盖到 {expected_start}，层有 {rows} 行",
            info.index
        )));
    }
    Ok(())
}

fn add(offset: usize, len: usize) -> Result<usize> {
    offset
        .checked_add(len)
        .ok_or_else(|| Error::Wire("索引偏移区间溢出".into()))
}

fn overflow(what: &str) -> Error {
    Error::Wire(format!("索引 {what} 长度计算溢出"))
}
