//! TTX3 级链与编码布局对账（第 45 轮，F3/F4）。
//!
//! 解析侧过去只读级目录、不校验尺寸链：0 尺寸幽灵级会带着 `tile_grid` 的
//! 零除/空循环混进视图；第二级比第一级更大也会被接受。更隐蔽的是
//! `level_count` 谎报——多出来的“级条目”其实是下一段的瓦片目录字节，
//! 只要字段凑巧可读就会变成幽灵级。编码端布局是封闭的：级目录紧接头部，
//! 每级瓦片目录与载荷按 [`super::level_storage_bytes`] 连续排布。这里按
//! 同一账本要求每个 `dir_offset` 等于预期游标，从而把谎报级数挡在解析期。
//!
//! 只复用现有错误变体：尺寸不合法走 `Ttx3Dims`，级链/目录偏移走
//! `Ttx3Level`；物理截断仍由头部检查与 `layout` 的载荷区间账报
//! `Ttx3Truncated`。

use super::{ArchiveLevel, HEADER_BYTES, LEVEL_ENTRY_BYTES, level_storage_bytes, validate_dims};
use crate::TextureError;

/// 校验级尺寸链与目录偏移（越界偏移留给 `layout` 报截断）。
pub(super) fn validate_chain(
    bytes_len: usize,
    levels: &[ArchiveLevel],
) -> Result<(), TextureError> {
    let mut expected = HEADER_BYTES as u64 + levels.len() as u64 * LEVEL_ENTRY_BYTES as u64;
    for (index, entry) in levels.iter().enumerate() {
        validate_dims(entry.width, entry.height)?;
        if index > 0 {
            let previous = &levels[index - 1];
            if entry.width > previous.width || entry.height > previous.height {
                return Err(TextureError::Ttx3Level {
                    level: index,
                    message: format!(
                        "级尺寸 {}×{} 不得大于上一级 {}×{}",
                        entry.width, entry.height, previous.width, previous.height
                    ),
                });
            }
            if entry.width == previous.width && entry.height == previous.height {
                return Err(TextureError::Ttx3Level {
                    level: index,
                    message: "相邻级尺寸完全相同".into(),
                });
            }
        }
        if entry.dir_offset != expected && entry.dir_offset < bytes_len as u64 {
            return Err(TextureError::Ttx3Level {
                level: index,
                message: format!(
                    "瓦片目录偏移 {} 与编码布局 {} 不符（级数谎报或目录被改写）",
                    entry.dir_offset, expected
                ),
            });
        }
        expected =
            expected.saturating_add(level_storage_bytes(entry.width, entry.height, entry.policy));
    }
    Ok(())
}
