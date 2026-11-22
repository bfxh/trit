//! 补丁事实：条数、成本、空操作与重复目标（含 tile / 行级编辑）。
//!
//! `noop` / `repeated` 的口径必须覆盖 `NegateRow` / `SetTile` / `FillTile`，
//! 否则「编辑了几位」的读数会虚高：tile 的每个坐标都参与 repeated 统计；
//! `NegateRow` 展开整行 trit + bias（不含 scale）。`SwapRows` 仍不展开
//! （交换两行的坐标并集没有"同一目标被重复写"的语义）。

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::edit::{Edit, Patch};
use crate::net::Net;
use crate::trit::Trit;

/// 补丁自身的事实（与行为无关的部分）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PatchFacts {
    /// 编辑条数。
    pub edits: usize,
    /// `SetTrit` 条数。
    pub trits: usize,
    /// `SetBias` 条数。
    pub biases: usize,
    /// `SetScale` 条数。
    pub scales: usize,
    /// 空操作条数：目标值已经等于当前值。
    pub noop: usize,
    /// 重复目标条数：同一位置被写第二次及以后。
    pub repeated: usize,
    /// 碰到的三值位置数。
    pub touched: usize,
}

impl PatchFacts {
    /// → JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "edits": self.edits,
            "trits": self.trits,
            "biases": self.biases,
            "scales": self.scales,
            "noop": self.noop,
            "repeated": self.repeated,
            "touched": self.touched,
        })
    }
}

/// 补丁事实：条数、成本、空操作与重复目标。
#[must_use]
pub fn patch_facts(base: &Net, patch: &Patch) -> PatchFacts {
    let cost = patch.cost(base);
    let mut seen: BTreeMap<(usize, u8, usize, usize), usize> = BTreeMap::new();
    let mut noop = 0usize;
    let mut repeated = 0usize;
    for edit in patch.edits() {
        if is_noop(base, edit) {
            noop += 1;
        }
        for_each_target(base, edit, |key| {
            let count = seen.entry(key).or_insert(0);
            if *count > 0 {
                repeated += 1;
            }
            *count += 1;
        });
    }
    PatchFacts {
        edits: cost.edits,
        trits: cost.trits,
        biases: cost.biases,
        scales: cost.scales,
        noop,
        repeated,
        touched: cost.touched,
    }
}

fn is_noop(net: &Net, edit: &Edit) -> bool {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
            ..
        } => net
            .layer(layer)
            .and_then(|one| one.trit(row, col))
            .is_some_and(|current| current == to),
        Edit::SetBias { layer, row, to, .. } => net
            .layer(layer)
            .and_then(|one| one.bias_trit(row))
            .is_some_and(|current| current == to),
        Edit::SetScale { layer, row, to, .. } => net
            .layer(layer)
            .and_then(|one| one.scale(row))
            .is_some_and(|current| current == to),
        Edit::SwapRows { layer, a, b } => {
            if a == b {
                return true;
            }
            net.layer(layer).is_some_and(|one| {
                a < one.rows()
                    && b < one.rows()
                    && one.bias_trit(a) == one.bias_trit(b)
                    && one.scale(a) == one.scale(b)
                    && (0..one.cols()).all(|col| one.trit(a, col) == one.trit(b, col))
            })
        }
        Edit::NegateRow { layer, row } => net.layer(layer).is_some_and(|one| {
            row < one.rows()
                && one.bias_trit(row) == Some(Trit::Zero)
                && (0..one.cols()).all(|col| one.trit(row, col) == Some(Trit::Zero))
        }),
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ref values,
        } => tile_is_noop(net, layer, row_start, rows, col_start, cols, |r, c| {
            values.get(r * cols + c).copied()
        }),
        Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            to,
        } => tile_is_noop(net, layer, row_start, rows, col_start, cols, |_, _| {
            Some(to)
        }),
    }
}

/// tile 目标是否与当前块逐位相同；越界或 `values` 长度不足都判非 noop。
fn tile_is_noop(
    net: &Net,
    layer: usize,
    row_start: usize,
    rows: usize,
    col_start: usize,
    cols: usize,
    target: impl Fn(usize, usize) -> Option<Trit>,
) -> bool {
    let Some(one) = net.layer(layer) else {
        return false;
    };
    let Some(row_end) = row_start.checked_add(rows) else {
        return false;
    };
    let Some(col_end) = col_start.checked_add(cols) else {
        return false;
    };
    if row_end > one.rows() || col_end > one.cols() {
        return false;
    }
    (0..rows).all(|r| {
        (0..cols).all(|c| {
            target(r, c).is_some_and(|want| one.trit(row_start + r, col_start + c) == Some(want))
        })
    })
}

/// 枚举一条编辑真正写入的坐标（用于 repeated 统计）。
fn for_each_target(net: &Net, edit: &Edit, mut f: impl FnMut((usize, u8, usize, usize))) {
    match *edit {
        Edit::SetTrit {
            layer, row, col, ..
        } => f((layer, 0, row, col)),
        Edit::SetBias { layer, row, .. } => f((layer, 1, row, 0)),
        Edit::SetScale { layer, row, .. } => f((layer, 2, row, 0)),
        Edit::NegateRow { layer, row } => {
            let Some(one) = net.layer(layer) else {
                return;
            };
            if row >= one.rows() {
                return;
            }
            for col in 0..one.cols() {
                f((layer, 0, row, col));
            }
            f((layer, 1, row, 0));
        }
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ..
        }
        | Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ..
        } => {
            for r in 0..rows {
                for c in 0..cols {
                    f((layer, 0, row_start + r, col_start + c));
                }
            }
        }
        Edit::SwapRows { .. } => {}
    }
}
