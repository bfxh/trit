//! 编辑的校验/应用与逐位撤销记录（从 `edit.rs` 拆出，控制单文件行数）。

use crate::error::{Error, Result};
use crate::net::Net;
use crate::trit::Trit;

use super::Edit;

#[derive(Clone, Debug)]
pub(crate) enum Undo {
    Trit {
        layer: usize,
        row: usize,
        col: usize,
        old: Trit,
    },
    Bias {
        layer: usize,
        row: usize,
        old: Trit,
    },
    Scale {
        layer: usize,
        row: usize,
        old: f32,
    },
    SwapRows {
        layer: usize,
        a: usize,
        b: usize,
    },
    NegateRow {
        layer: usize,
        row: usize,
    },
    Tile {
        layer: usize,
        row_start: usize,
        rows: usize,
        col_start: usize,
        cols: usize,
        old: Vec<Trit>,
    },
}

fn layer_or_err(net: &Net, layer: usize) -> Result<&crate::net::Layer> {
    net.layer(layer).ok_or(Error::OutOfBounds {
        layer,
        row: 0,
        col: 0,
    })
}

fn validate_tile(
    net: &Net,
    layer: usize,
    row_start: usize,
    rows: usize,
    col_start: usize,
    cols: usize,
) -> Result<usize> {
    let target = layer_or_err(net, layer)?;
    if rows == 0 || cols == 0 {
        return Err(Error::Wire("tile 行列不能为 0".into()));
    }
    let row_end = row_start
        .checked_add(rows)
        .ok_or_else(|| Error::Wire("tile 行范围溢出".into()))?;
    let col_end = col_start
        .checked_add(cols)
        .ok_or_else(|| Error::Wire("tile 列范围溢出".into()))?;
    if row_end > target.rows() || col_end > target.cols() {
        return Err(Error::OutOfBounds {
            layer,
            row: row_end.saturating_sub(1),
            col: col_end.saturating_sub(1),
        });
    }
    rows.checked_mul(cols)
        .ok_or_else(|| Error::Wire("tile 元素数溢出".into()))
}

pub(crate) fn validate_one(net: &Net, edit: &Edit) -> Result<()> {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to: _,
        } => {
            let target = layer_or_err(net, layer)?;
            if row >= target.rows() || col >= target.cols() {
                return Err(Error::OutOfBounds { layer, row, col });
            }
        }
        Edit::SetBias { layer, row, to: _ } => {
            let target = layer_or_err(net, layer)?;
            if row >= target.rows() {
                return Err(Error::OutOfBounds { layer, row, col: 0 });
            }
        }
        Edit::SetScale { layer, row, to } => {
            let target = layer_or_err(net, layer)?;
            if row >= target.rows() {
                return Err(Error::OutOfBounds { layer, row, col: 0 });
            }
            if !to.is_finite() || to <= 0.0 {
                return Err(Error::InvalidScale { row, value: to });
            }
        }
        Edit::SwapRows { layer, a, b } => {
            let target = layer_or_err(net, layer)?;
            if a >= target.rows() || b >= target.rows() {
                return Err(Error::OutOfBounds {
                    layer,
                    row: a.max(b),
                    col: 0,
                });
            }
        }
        Edit::NegateRow { layer, row } => {
            let target = layer_or_err(net, layer)?;
            if row >= target.rows() {
                return Err(Error::OutOfBounds { layer, row, col: 0 });
            }
        }
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ref values,
        } => {
            let elements = validate_tile(net, layer, row_start, rows, col_start, cols)?;
            if values.len() != elements {
                return Err(Error::Shape {
                    what: "tile.values",
                    expected: elements,
                    got: values.len(),
                });
            }
        }
        Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            to: _,
        } => {
            validate_tile(net, layer, row_start, rows, col_start, cols)?;
        }
    }
    Ok(())
}

fn snapshot(
    net: &Net,
    layer: usize,
    row_start: usize,
    rows: usize,
    col_start: usize,
    cols: usize,
) -> Vec<Trit> {
    let target = net.layer(layer).expect("validated");
    let mut out = Vec::with_capacity(rows * cols);
    for row in 0..rows {
        for col in 0..cols {
            out.push(
                target
                    .trit(row_start + row, col_start + col)
                    .expect("validated"),
            );
        }
    }
    out
}

pub(crate) fn apply_one(net: &mut Net, edit: &Edit) -> Undo {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => {
            let old = net
                .layer(layer)
                .and_then(|l| l.trit(row, col))
                .expect("validated");
            net.layer_mut(layer).set_trit(row, col, to);
            Undo::Trit {
                layer,
                row,
                col,
                old,
            }
        }
        Edit::SetBias { layer, row, to } => {
            let old = net
                .layer(layer)
                .and_then(|l| l.bias_trit(row))
                .expect("validated");
            net.layer_mut(layer).set_bias(row, to);
            Undo::Bias { layer, row, old }
        }
        Edit::SetScale { layer, row, to } => {
            let old = net
                .layer(layer)
                .and_then(|l| l.scale(row))
                .expect("validated");
            net.layer_mut(layer).set_scale(row, to);
            Undo::Scale { layer, row, old }
        }
        Edit::SwapRows { layer, a, b } => {
            net.layer_mut(layer).swap_rows(a, b);
            Undo::SwapRows { layer, a, b }
        }
        Edit::NegateRow { layer, row } => {
            net.layer_mut(layer).negate_row(row);
            Undo::NegateRow { layer, row }
        }
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ref values,
        } => {
            let old = snapshot(net, layer, row_start, rows, col_start, cols);
            let target = net.layer_mut(layer);
            for row in 0..rows {
                for col in 0..cols {
                    target.set_trit(row_start + row, col_start + col, values[row * cols + col]);
                }
            }
            Undo::Tile {
                layer,
                row_start,
                rows,
                col_start,
                cols,
                old,
            }
        }
        Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            to,
        } => {
            let old = snapshot(net, layer, row_start, rows, col_start, cols);
            let target = net.layer_mut(layer);
            for row in 0..rows {
                for col in 0..cols {
                    target.set_trit(row_start + row, col_start + col, to);
                }
            }
            Undo::Tile {
                layer,
                row_start,
                rows,
                col_start,
                cols,
                old,
            }
        }
    }
}
