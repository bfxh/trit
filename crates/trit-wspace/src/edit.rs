//! 权重编辑：补丁是**事务**，失败不改工件；日志可以逐位回滚。
//!
//! 一条编辑只表达“把某个三值改成另一个三值”这类原子动作。批量编辑（[`Patch`]）
//! 先整体校验再整体应用，应用失败不会留下半改状态；[`Journal`] 记录每一项的前值，
//! 可以精确撤销（不是“再算一遍”）。

use crate::error::{Error, Result};
use crate::net::Net;
use crate::trit::Trit;

mod ops;
use ops::Undo;

/// 原子编辑。
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// 改一个权重。
    SetTrit {
        /// 层号。
        layer: usize,
        /// 行号。
        row: usize,
        /// 列号。
        col: usize,
        /// 目标值。
        to: Trit,
    },
    /// 改一个偏置。
    SetBias {
        /// 层号。
        layer: usize,
        /// 行号。
        row: usize,
        /// 目标值。
        to: Trit,
    },
    /// 改一行 scale（不改行为，只改 f32 读数）。
    SetScale {
        /// 层号。
        layer: usize,
        /// 行号。
        row: usize,
        /// 目标值，必须是有限正数。
        to: f32,
    },
    /// 交换两行（权重、偏置、scale 一起）。
    SwapRows {
        /// 层号。
        layer: usize,
        /// 第一行。
        a: usize,
        /// 第二行。
        b: usize,
    },
    /// 整行取反（权重与偏置一起）。
    NegateRow {
        /// 层号。
        layer: usize,
        /// 行号。
        row: usize,
    },
    /// 把矩形块设为给定 trits（行主序）。
    SetTile {
        /// 层号。
        layer: usize,
        /// 起始行。
        row_start: usize,
        /// 行数。
        rows: usize,
        /// 起始列。
        col_start: usize,
        /// 列数。
        cols: usize,
        /// `rows × cols` 个目标值（行主序）。
        values: Vec<Trit>,
    },
    /// 用同一个 trit 填充矩形块。
    FillTile {
        /// 层号。
        layer: usize,
        /// 起始行。
        row_start: usize,
        /// 行数。
        rows: usize,
        /// 起始列。
        col_start: usize,
        /// 列数。
        cols: usize,
        /// 目标值。
        to: Trit,
    },
}

/// 一组按顺序应用的编辑。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Patch {
    edits: Vec<Edit>,
}

/// 编辑日志；逆序播放即精确撤销。
#[derive(Clone, Debug)]
pub struct Journal {
    entries: Vec<Undo>,
}

/// 改动量：`touched` 是“碰到的三值位置数”（含偏置），作为最小编辑的主指标。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    /// 编辑条数。
    pub edits: usize,
    /// `SetTrit` 条数。
    pub trits: usize,
    /// `SetBias` 条数。
    pub biases: usize,
    /// `SetScale` 条数。
    pub scales: usize,
    /// 行级操作条数（`SwapRows` / `NegateRow`）。
    pub row_ops: usize,
    /// tile 级操作条数（`SetTile` / `FillTile`）。
    pub tile_ops: usize,
    /// 碰到的三值位置数（scale 不计入）。
    pub touched: usize,
}

impl Journal {
    /// 逆序撤销，恢复到 [`Patch::apply`] 之前的状态。
    pub fn undo(self, net: &mut Net) {
        for undo in self.entries.into_iter().rev() {
            match undo {
                Undo::Trit {
                    layer,
                    row,
                    col,
                    old,
                } => net.layer_mut(layer).set_trit(row, col, old),
                Undo::Bias { layer, row, old } => net.layer_mut(layer).set_bias(row, old),
                Undo::Scale { layer, row, old } => net.layer_mut(layer).set_scale(row, old),
                Undo::SwapRows { layer, a, b } => net.layer_mut(layer).swap_rows(a, b),
                Undo::NegateRow { layer, row } => net.layer_mut(layer).negate_row(row),
                Undo::Tile {
                    layer,
                    row_start,
                    rows,
                    col_start,
                    cols,
                    old,
                } => {
                    let target = net.layer_mut(layer);
                    for row in 0..rows {
                        for col in 0..cols {
                            target.set_trit(
                                row_start + row,
                                col_start + col,
                                old[row * cols + col],
                            );
                        }
                    }
                }
            }
        }
    }
}

impl Patch {
    /// 从编辑列表构造。
    #[must_use]
    pub fn new(edits: Vec<Edit>) -> Self {
        Self { edits }
    }

    /// 全部编辑（按应用顺序）。
    #[must_use]
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }

    /// 编辑条数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.edits.len()
    }

    /// 是否空补丁。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }

    /// 追加一条（搜索用）。
    pub(crate) fn push(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    /// 整体校验：任何一条非法就报错，不做任何修改。
    pub fn validate(&self, net: &Net) -> Result<()> {
        for edit in &self.edits {
            ops::validate_one(net, edit)?;
        }
        Ok(())
    }

    /// 事务式应用：先整体校验，再逐条应用；返回可撤销日志。
    pub fn apply(&self, net: &mut Net) -> Result<Journal> {
        self.validate(net)?;
        let mut entries = Vec::with_capacity(self.edits.len());
        for edit in &self.edits {
            entries.push(ops::apply_one(net, edit));
        }
        Ok(Journal { entries })
    }

    /// 改动量。行级操作需要网络形状，所以必须传 `net`。
    #[must_use]
    pub fn cost(&self, net: &Net) -> Cost {
        let mut cost = Cost {
            edits: self.edits.len(),
            ..Cost::default()
        };
        for edit in &self.edits {
            match *edit {
                Edit::SetTrit { .. } => {
                    cost.trits += 1;
                    cost.touched += 1;
                }
                Edit::SetBias { .. } => {
                    cost.biases += 1;
                    cost.touched += 1;
                }
                Edit::SetScale { .. } => cost.scales += 1,
                Edit::SwapRows { layer, .. } => {
                    cost.row_ops += 1;
                    let cols = net.layer(layer).map_or(0, |l| l.cols());
                    cost.touched += 2 * cols + 2;
                }
                Edit::NegateRow { layer, .. } => {
                    cost.row_ops += 1;
                    let cols = net.layer(layer).map_or(0, |l| l.cols());
                    cost.touched += cols + 1;
                }
                Edit::SetTile { rows, cols, .. } | Edit::FillTile { rows, cols, .. } => {
                    cost.tile_ops += 1;
                    cost.touched += rows.saturating_mul(cols);
                }
            }
        }
        cost
    }

    /// 两个同架构网络之间的最小差集（只含逐点编辑）。
    pub fn diff(from: &Net, to: &Net) -> Result<Self> {
        if from.arch() != to.arch() {
            return Err(Error::ArchMismatch);
        }
        let mut edits = Vec::new();
        for (index, (a, b)) in from.layers().iter().zip(to.layers()).enumerate() {
            for row in 0..a.rows() {
                for col in 0..a.cols() {
                    let left = a.trit(row, col).expect("row/col in range");
                    let right = b.trit(row, col).expect("row/col in range");
                    if left != right {
                        edits.push(Edit::SetTrit {
                            layer: index,
                            row,
                            col,
                            to: right,
                        });
                    }
                }
                let left_bias = a.bias_trit(row).expect("row in range");
                let right_bias = b.bias_trit(row).expect("row in range");
                if left_bias != right_bias {
                    edits.push(Edit::SetBias {
                        layer: index,
                        row,
                        to: right_bias,
                    });
                }
                let left_scale = a.scale(row).expect("row in range");
                let right_scale = b.scale(row).expect("row in range");
                if left_scale != right_scale {
                    edits.push(Edit::SetScale {
                        layer: index,
                        row,
                        to: right_scale,
                    });
                }
            }
        }
        Ok(Self { edits })
    }

    /// 是否包含同坐标的另一条编辑（搜索时避免原地打转）。
    pub(crate) fn same_coord(&self, other: &Edit) -> bool {
        self.edits.last().is_some_and(|last| last.same_coord(other))
    }
}

impl Edit {
    /// 单条应用（内部搜索用）；失败时网络不变。
    pub(crate) fn apply_to(&self, net: &mut Net) -> Result<()> {
        ops::validate_one(net, self)?;
        ops::apply_one(net, self);
        Ok(())
    }

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
            _ => false,
        }
    }
}
