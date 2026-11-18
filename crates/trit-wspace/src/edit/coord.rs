//! 编辑坐标判等（从 `edit.rs` 拆出，控制单文件行数）。

use super::Edit;

impl Edit {
    pub(crate) fn same_coord(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::SetTrit {
                    layer: l1,
                    row: r1,
                    col: c1,
                    ..
                },
                Self::SetTrit {
                    layer: l2,
                    row: r2,
                    col: c2,
                    ..
                },
            ) => l1 == l2 && r1 == r2 && c1 == c2,
            (
                Self::SetBias {
                    layer: l1, row: r1, ..
                },
                Self::SetBias {
                    layer: l2, row: r2, ..
                },
            ) => l1 == l2 && r1 == r2,
            (
                Self::SetScale {
                    layer: l1, row: r1, ..
                },
                Self::SetScale {
                    layer: l2, row: r2, ..
                },
            ) => l1 == l2 && r1 == r2,
            (Self::NegateRow { layer: l1, row: r1 }, Self::NegateRow { layer: l2, row: r2 }) => {
                l1 == l2 && r1 == r2
            }
            (
                Self::SwapRows {
                    layer: l1,
                    a: a1,
                    b: b1,
                },
                Self::SwapRows {
                    layer: l2,
                    a: a2,
                    b: b2,
                },
            ) => l1 == l2 && ((a1 == a2 && b1 == b2) || (a1 == b2 && b1 == a2)),
            _ => false,
        }
    }
}
