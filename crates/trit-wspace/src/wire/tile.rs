//! tile 编辑的 JSON 编解码（从 `wire/mod.rs` 拆出，控制单文件行数）。

use serde_json::{Value, json};

use crate::edit::Edit;
use crate::error::Result;

use super::{field_trit, field_usize, row_trits, wire};

pub(super) fn to_value(edit: &Edit) -> Value {
    match edit {
        Edit::SetTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            values,
        } => json!({
            "op": "set_tile",
            "layer": layer,
            "row_start": row_start,
            "rows": rows,
            "col_start": col_start,
            "cols": cols,
            "values": values.iter().map(|trit| trit.to_char()).collect::<String>(),
        }),
        Edit::FillTile {
            layer,
            row_start,
            rows,
            cols,
            col_start,
            to,
        } => json!({
            "op": "fill_tile",
            "layer": layer,
            "row_start": row_start,
            "rows": rows,
            "col_start": col_start,
            "cols": cols,
            "to": to.to_char().to_string(),
        }),
        _ => unreachable!("只有 tile 编辑"),
    }
}

pub(super) fn from_value(op: &str, value: &Value) -> Result<Edit> {
    match op {
        "set_tile" => {
            let rows = field_usize(value, "rows")?;
            let cols = field_usize(value, "cols")?;
            let values = tile_values(value, rows, cols)?;
            Ok(Edit::SetTile {
                layer: field_usize(value, "layer")?,
                row_start: field_usize(value, "row_start")?,
                rows,
                col_start: field_usize(value, "col_start")?,
                cols,
                values,
            })
        }
        "fill_tile" => Ok(Edit::FillTile {
            layer: field_usize(value, "layer")?,
            row_start: field_usize(value, "row_start")?,
            rows: field_usize(value, "rows")?,
            col_start: field_usize(value, "col_start")?,
            cols: field_usize(value, "cols")?,
            to: field_trit(value, "to")?,
        }),
        other => Err(wire(format!("未知 tile edit.op `{other}`"))),
    }
}

fn tile_values(value: &Value, rows: usize, cols: usize) -> Result<Vec<crate::trit::Trit>> {
    let expected = rows
        .checked_mul(cols)
        .ok_or_else(|| wire("tile 元素数溢出"))?;
    let raw = value
        .get("values")
        .ok_or_else(|| wire("set_tile 缺 values"))?;
    let values = row_trits(raw)?;
    if values.len() != expected {
        return Err(wire(format!(
            "values 长度 {} != rows*cols {expected}",
            values.len()
        )));
    }
    Ok(values)
}
