//! 张量数据区全局区间校验（#625）。
//!
//! 每个张量在对齐后的数据段里占 `[offset, offset + elements × element_size)`。
//! 导入前统一检查：区间不越出数据段；按 offset 排序后相邻区间不得相交（首尾相接
//! `[a,b)` 与 `[b,c)` 允许）。相交时错误点名冲突的两个张量。

use super::{GgufError, TensorInfo};

/// 校验全部张量的数据区间：不越界、互不相交。
pub(crate) fn validate_regions(tensors: &[TensorInfo], data: &[u8]) -> Result<(), GgufError> {
    let mut regions: Vec<(u64, u64, &str)> = Vec::with_capacity(tensors.len());
    for tensor in tensors {
        let elements = tensor.elements()?;
        let element_size = tensor
            .element_size()
            .ok_or_else(|| GgufError::UnsupportedType {
                tensor: tensor.name.clone(),
                type_id: tensor.type_id,
            })?;
        let needed = elements
            .checked_mul(element_size as u64)
            .ok_or(GgufError::Truncated)?;
        let start = tensor.offset;
        let end = start.checked_add(needed).ok_or(GgufError::Truncated)?;
        if end > data.len() as u64 {
            return Err(GgufError::Truncated);
        }
        regions.push((start, end, tensor.name.as_str()));
    }
    regions.sort_by_key(|(start, _, _)| *start);
    for pair in regions.windows(2) {
        let (_, first_end, first) = pair[0];
        let (second_start, _, second) = pair[1];
        if second_start < first_end {
            return Err(GgufError::Overlap {
                first: first.to_string(),
                second: second.to_string(),
            });
        }
    }
    Ok(())
}
