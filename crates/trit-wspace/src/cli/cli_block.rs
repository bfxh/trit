//! CLI `index-artifact` / `read-block`：WSP2 块索引与随机读取。

use std::fs;

use serde_json::{Value, json};

use crate::Net;
use crate::wsp2::BlockIndex;

use super::{atomic_write, emit, err, one, parse, value, write_out};

/// `index-artifact <artifact.wsp2> --out <idx>`：扫描 WSP2，生成块索引 JSON。
pub(super) fn index_artifact(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "index-artifact <artifact.wsp2> --out <idx>")?;
    let out = value(&parsed, "out")?;
    let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
    let index = BlockIndex::scan(&bytes).map_err(err)?;
    let text = serde_json::to_string_pretty(&index.to_json())
        .map_err(|error| format!("索引 JSON 序列化失败：{error}"))?;
    atomic_write(out, &text)?;
    emit(&json!({
        "source": path,
        "out": out,
        "source_bytes": bytes.len(),
        "layers": index.layers.len(),
        "blocks": index.layers.iter().map(|layer| layer.row_blocks.len()).sum::<usize>(),
        "block_rows": index.block_rows,
        "index_bytes": text.len(),
    }))
}

/// `read-block <artifact.wsp2> --index <idx> --layer N [--row-start R --rows K] [--out <path>]`。
pub(super) fn read_block(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["verify"])?;
    let path = one(
        &parsed,
        "read-block <artifact.wsp2> --index <idx> --layer N [--row-start R --rows K] [--out <path>]",
    )?;
    let index_path = value(&parsed, "index")?;
    let layer_index = value(&parsed, "layer")?
        .parse::<usize>()
        .map_err(|_| "--layer 需要非负整数".to_string())?;
    let mut file = fs::File::open(path).map_err(|error| format!("{path}: {error}"))?;
    let text = fs::read_to_string(index_path).map_err(|error| format!("{index_path}: {error}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("{index_path} JSON 非法：{error}"))?;
    let index = BlockIndex::from_json(&value).map_err(err)?;
    if parsed.switches.contains("verify") {
        let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        let scanned = BlockIndex::scan(&bytes).map_err(err)?;
        if scanned != index {
            return Err("索引与源文件不一致（--verify）".into());
        }
    }
    let info = index.layers.get(layer_index).ok_or_else(|| {
        format!(
            "层号 {layer_index} 越界（索引共 {} 层）",
            index.layers.len()
        )
    })?;
    let row_start = parsed.values.get("row-start").map_or(Ok(0), |raw| {
        raw.parse::<usize>()
            .map_err(|_| format!("--row-start 需要非负整数，得到 `{raw}`"))
    })?;
    if row_start > info.rows {
        return Err(format!("--row-start {row_start} 超过层行数 {}", info.rows));
    }
    let rows = match parsed.values.get("rows") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| format!("--rows 需要非负整数，得到 `{raw}`"))?,
        None => info.rows - row_start,
    };
    let layer = if parsed.switches.contains("verify") {
        let bytes = fs::read(path).map_err(|error| format!("{path}: {error}"))?;
        index.read_rows(&bytes, layer_index, row_start, rows)
    } else {
        index.read_rows_io(&mut file, layer_index, row_start, rows)
    }
    .map_err(err)?;
    let net = Net::new(vec![layer]).map_err(err)?;
    write_out(&parsed, &net)?;
    emit(&json!({
        "layer": layer_index,
        "row_start": row_start,
        "rows": rows,
        "cols": info.cols,
        "layer_bytes": info.end_offset - info.layer_offset,
        "out": parsed.values.get("out"),
    }))
}
