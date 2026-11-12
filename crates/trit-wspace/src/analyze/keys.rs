//! 编辑排序键（从 `analyze.rs` 拆出，控制单文件行数）。

use crate::edit::Edit;

pub(crate) fn edit_key(edit: &Edit) -> (u8, usize, usize, usize, i64) {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => (0, layer, row, col, i64::from(to.to_i8())),
        Edit::SetBias { layer, row, to } => (1, layer, row, 0, i64::from(to.to_i8())),
        Edit::SetScale { layer, row, to } => (2, layer, row, 0, i64::from(to.to_bits())),
        Edit::SwapRows { layer, a, b } => (3, layer, a, b, 0),
        Edit::NegateRow { layer, row } => (4, layer, row, 0, 0),
        Edit::SetTile {
            layer,
            row_start,
            col_start,
            ..
        } => (5, layer, row_start, col_start, 0),
        Edit::FillTile {
            layer,
            row_start,
            col_start,
            to,
            ..
        } => (6, layer, row_start, col_start, i64::from(to.to_i8())),
    }
}
