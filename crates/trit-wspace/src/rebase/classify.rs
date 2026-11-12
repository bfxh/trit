//! 三方分类（从 `rebase.rs` 拆出，控制单文件行数）。

use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::net::{Layer, Net};

use super::Verdict;

pub(super) fn classify(base: &Net, target: &Net, edit: &Edit) -> Result<Verdict> {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => {
            let before = trit_at(base, layer, row, col)?;
            let now = trit_at(target, layer, row, col)?;
            if before == now {
                Ok(Verdict::Clean)
            } else if now == to {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: before.to_string(),
                    target: now.to_string(),
                })
            }
        }
        Edit::SetBias { layer, row, to } => {
            let before = bias_at(base, layer, row)?;
            let now = bias_at(target, layer, row)?;
            if before == now {
                Ok(Verdict::Clean)
            } else if now == to {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: before.to_string(),
                    target: now.to_string(),
                })
            }
        }
        Edit::SetScale { layer, row, to } => {
            let before = scale_at(base, layer, row)?;
            let now = scale_at(target, layer, row)?;
            if before.to_bits() == now.to_bits() {
                Ok(Verdict::Clean)
            } else if now.to_bits() == to.to_bits() {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: before.to_string(),
                    target: now.to_string(),
                })
            }
        }
        Edit::SwapRows { layer, a, b } => {
            if a == b {
                return Ok(Verdict::Clean);
            }
            let before = layer_at(base, layer)?;
            let now = layer_at(target, layer)?;
            if row_eq(before, a, now, a) && row_eq(before, b, now, b) {
                Ok(Verdict::Clean)
            } else if row_eq(before, a, now, b) && row_eq(before, b, now, a) {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: row_pair(before, a, b),
                    target: row_pair(now, a, b),
                })
            }
        }
        Edit::NegateRow { layer, row } => {
            let before = layer_at(base, layer)?;
            let now = layer_at(target, layer)?;
            if row_eq(before, row, now, row) {
                Ok(Verdict::Clean)
            } else if row_eq_negated(before, row, now, row) {
                Ok(Verdict::AlreadyApplied)
            } else {
                Ok(Verdict::Conflict {
                    base: row_fingerprint(before, row),
                    target: row_fingerprint(now, row),
                })
            }
        }
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            ref values,
        } => classify_tile(
            base, target, layer, row_start, rows, col_start, cols, values,
        ),
        Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            to,
        } => {
            let elements = rows
                .checked_mul(cols)
                .ok_or_else(|| Error::Wire("tile 元素数溢出".into()))?;
            classify_tile(
                base,
                target,
                layer,
                row_start,
                rows,
                col_start,
                cols,
                &vec![to; elements],
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn classify_tile(
    base: &Net,
    target: &Net,
    layer: usize,
    row_start: usize,
    rows: usize,
    col_start: usize,
    cols: usize,
    wanted: &[crate::trit::Trit],
) -> Result<Verdict> {
    let before = tile_at(base, layer, row_start, rows, col_start, cols)?;
    let now = tile_at(target, layer, row_start, rows, col_start, cols)?;
    if before == now {
        Ok(Verdict::Clean)
    } else if now == wanted {
        Ok(Verdict::AlreadyApplied)
    } else {
        Ok(Verdict::Conflict {
            base: tile_text(&before),
            target: tile_text(&now),
        })
    }
}

fn tile_at(
    net: &Net,
    layer: usize,
    row_start: usize,
    rows: usize,
    col_start: usize,
    cols: usize,
) -> Result<Vec<crate::trit::Trit>> {
    let target = layer_at(net, layer)?;
    let elements = rows
        .checked_mul(cols)
        .ok_or_else(|| Error::Wire("tile 元素数溢出".into()))?;
    let mut out = Vec::with_capacity(elements);
    for row in 0..rows {
        for col in 0..cols {
            let row_index = row_start
                .checked_add(row)
                .ok_or_else(|| Error::Wire("tile 行范围溢出".into()))?;
            let col_index = col_start
                .checked_add(col)
                .ok_or_else(|| Error::Wire("tile 列范围溢出".into()))?;
            out.push(
                target
                    .trit(row_index, col_index)
                    .ok_or(Error::OutOfBounds {
                        layer,
                        row: row_index,
                        col: col_index,
                    })?,
            );
        }
    }
    Ok(out)
}

fn tile_text(values: &[crate::trit::Trit]) -> String {
    values.iter().map(|trit| trit.to_char()).collect()
}

fn layer_at(net: &Net, layer: usize) -> Result<&Layer> {
    net.layer(layer).ok_or(Error::OutOfBounds {
        layer,
        row: 0,
        col: 0,
    })
}

fn trit_at(net: &Net, layer: usize, row: usize, col: usize) -> Result<crate::trit::Trit> {
    layer_at(net, layer)?
        .trit(row, col)
        .ok_or(Error::OutOfBounds { layer, row, col })
}

fn bias_at(net: &Net, layer: usize, row: usize) -> Result<crate::trit::Trit> {
    layer_at(net, layer)?
        .bias_trit(row)
        .ok_or(Error::OutOfBounds { layer, row, col: 0 })
}

fn scale_at(net: &Net, layer: usize, row: usize) -> Result<f32> {
    layer_at(net, layer)?
        .scale(row)
        .ok_or(Error::OutOfBounds { layer, row, col: 0 })
}

fn row_eq(left: &Layer, left_row: usize, right: &Layer, right_row: usize) -> bool {
    left.row(left_row) == right.row(right_row)
        && left.bias_trit(left_row) == right.bias_trit(right_row)
        && left.scale(left_row).map(f32::to_bits) == right.scale(right_row).map(f32::to_bits)
}

fn row_eq_negated(left: &Layer, left_row: usize, right: &Layer, right_row: usize) -> bool {
    let same_weights = match (left.row(left_row), right.row(right_row)) {
        (Some(a), Some(b)) => a.iter().map(|trit| trit.neg()).eq(b.iter().copied()),
        _ => false,
    };
    same_weights
        && left.bias_trit(left_row).map(|trit| trit.neg()) == right.bias_trit(right_row)
        && left.scale(left_row).map(f32::to_bits) == right.scale(right_row).map(f32::to_bits)
}

fn row_pair(layer: &Layer, a: usize, b: usize) -> String {
    format!(
        "{}/{}",
        row_fingerprint(layer, a),
        row_fingerprint(layer, b)
    )
}

fn row_fingerprint(layer: &Layer, row: usize) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    if let Some(trits) = layer.row(row) {
        for trit in trits {
            hash = fnv(hash, &[trit.to_i8() as u8]);
        }
    }
    if let Some(bias) = layer.bias_trit(row) {
        hash = fnv(hash, &[bias.to_i8() as u8]);
    }
    if let Some(scale) = layer.scale(row) {
        hash = fnv(hash, &scale.to_bits().to_le_bytes());
    }
    format!("{hash:016x}")
}

fn fnv(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
