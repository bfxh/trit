//! TTX3 解析期布局对账（第 40 轮 FIX-T，F1/F2）。
//!
//! 修前解析侧不维护各级瓦片目录与载荷的区间账本：条目偏移被当作可信指针，
//! CRC 只认证「切片 ↔ 本条目的 stored crc」，不绑定级别身份——L1 条目
//! `off1` 指回 L0 合法载荷并沿用 L0 CRC 时，解码器静默解出 L0 的颜色
//! （跨级色值错位）。同时政策档位字节与物理副本数不对账：Capacity 挂非零
//! `off2`、Strength 缺第二副本、政策双向翻转都被接受，`copies()` 账务与
//! 磁盘布局脱节。
//!
//! 本闸门在 [`super::Ttx3Archive::parse`] 阶段一次性对账：
//! - 条目载荷半开区间 `[off, off+len)` 必须落在本级数据区内，不得指回其它级别；
//! - 全部副本区间两两互不重叠（恰好相接放行）；
//! - 政策档位与物理副本数一致：Capacity 无第二副本偏移，Strength 第二副本非零。
//!
//! 复用既有 [`TextureError`] 变体，不新增错误类型。

use super::{ArchiveLevel, LevelPolicy, TILE_DATA_BYTES, TILE_ENTRY_BYTES, tile_grid};
use crate::TextureError;

/// 对账所有级的瓦片目录条目与载荷区间。
pub(super) fn validate(bytes: &[u8], levels: &[ArchiveLevel]) -> Result<(), TextureError> {
    let file_len = bytes.len() as u64;
    // (start, end, level, tile_index) 全局区间台账。
    let mut intervals: Vec<(u64, u64, usize, usize)> = Vec::new();

    for (level, entry) in levels.iter().enumerate() {
        let (tiles_x, tiles_y) = tile_grid(entry.width, entry.height);
        let tiles = tiles_x * tiles_y;
        // 本级数据区 = 瓦片目录之后、按政策份数计账的连续载荷段。
        let data_start = entry
            .dir_offset
            .saturating_add((tiles * TILE_ENTRY_BYTES) as u64);
        let data_end =
            data_start.saturating_add((tiles * TILE_DATA_BYTES * entry.policy.copies()) as u64);

        for tile_index in 0..tiles {
            let tx = tile_index % tiles_x;
            let ty = tile_index / tiles_x;
            let entry_end = entry
                .dir_offset
                .saturating_add((tile_index as u64 + 1) * TILE_ENTRY_BYTES as u64);
            if entry_end > file_len {
                return Err(TextureError::Ttx3Truncated {
                    needed: entry_end,
                    got: file_len,
                });
            }
            let at = entry_end as usize - TILE_ENTRY_BYTES;
            let off1 = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
            let off2 = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
            let len = u32::from_le_bytes(bytes[at + 16..at + 20].try_into().unwrap());

            if len as usize != TILE_DATA_BYTES {
                return Err(TextureError::Ttx3Tile {
                    level,
                    tx,
                    ty,
                    message: format!("瓦片长度 {len} 字节，应为 {TILE_DATA_BYTES}"),
                });
            }
            match entry.policy {
                LevelPolicy::Capacity if off2 != 0 => {
                    return Err(TextureError::Ttx3Tile {
                        level,
                        tx,
                        ty,
                        message: "Capacity 档声明单份冗余，条目却挂了第二副本偏移".into(),
                    });
                }
                LevelPolicy::Strength if off2 == 0 => {
                    return Err(TextureError::Ttx3Tile {
                        level,
                        tx,
                        ty,
                        message: "Strength 档声明双份冗余，条目第二副本偏移为零".into(),
                    });
                }
                _ => {}
            }

            for offset in [off1, off2] {
                if offset == 0 {
                    continue;
                }
                let end = offset.saturating_add(len as u64);
                if end > file_len {
                    return Err(TextureError::Ttx3Truncated {
                        needed: end,
                        got: file_len,
                    });
                }
                if offset < data_start || end > data_end {
                    return Err(TextureError::Ttx3Tile {
                        level,
                        tx,
                        ty,
                        message: "载荷区间不属于本级数据区（跨级别名或越界）".into(),
                    });
                }
                intervals.push((offset, end, level, tile_index));
            }
        }
    }

    // 按起点排序后用运行最大终点判相交：半开区间 `[start, end)`，
    // 终点 == 下一区间起点（首尾相接）不相交，放行。
    intervals.sort_unstable_by_key(|&(start, _, _, _)| start);
    let mut max_end = 0u64;
    for &(start, end, level, tile_index) in &intervals {
        if start < max_end {
            let (tiles_x, _) = tile_grid(levels[level].width, levels[level].height);
            return Err(TextureError::Ttx3Tile {
                level,
                tx: tile_index % tiles_x,
                ty: tile_index / tiles_x,
                message: "载荷区间与其它瓦片/副本重叠（跨级或跨瓦片别名）".into(),
            });
        }
        max_end = max_end.max(end);
    }
    Ok(())
}
