//! CLI `list-safetensors` / `import-safetensors` / `export-safetensors`：
//! 真实权重张量 ↔ 三值 WSP1（从 `cli.rs` 拆出，控制单文件行数）。

use serde_json::{Value, json};

use crate::safetensors;
use crate::to_wsp;

use super::{emit, err, load_net, one, parse, value, write_out};

pub(super) fn list(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "list-safetensors <file>")?;
    let tensors: Vec<Value> = safetensors::list(path)
        .map_err(err)?
        .into_iter()
        .map(|tensor| {
            json!({
                "name": tensor.name,
                "dtype": tensor.dtype.name(),
                "shape": tensor.shape,
                "bytes": tensor.offsets.1.saturating_sub(tensor.offsets.0),
            })
        })
        .collect();
    emit(&json!({ "tensors": tensors }))
}

pub(super) fn import(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "import-safetensors <file> --tensor <name> [--out <path>]",
    )?;
    let name = value(&parsed, "tensor")?;
    let weights = safetensors::read_tensor(path, name).map_err(err)?;
    let layer = weights.to_layer().map_err(err)?;
    let net = crate::Net::new(vec![layer]).map_err(err)?;
    write_out(&parsed, &net)?;
    emit(&json!({
        "tensor": {
            "name": name,
            "rows": weights.rows,
            "cols": weights.cols,
            "values": weights.values.len(),
        },
        "summary": crate::wire::net_summary(&net),
        "wsp1": to_wsp(&net),
    }))
}

pub(super) fn export(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "export-safetensors <artifact|-> --out <path> [--tensor <name>]",
    )?;
    let out = value(&parsed, "out")?;
    let name = parsed
        .values
        .get("tensor")
        .map_or("weights", String::as_str);
    let net = load_net(path)?;
    if net.layers().len() != 1 {
        return Err(format!(
            "export-safetensors 只支持单层网络（当前 {} 层）",
            net.layers().len()
        ));
    }
    let layer = &net.layers()[0];
    let (rows, cols) = (layer.rows(), layer.cols());
    let mut values = Vec::with_capacity(rows * cols);
    for row in 0..rows {
        let scale = layer.scale(row).expect("行在范围内");
        for col in 0..cols {
            let trit = layer.trit(row, col).expect("列在范围内");
            values.push(f32::from(trit.to_i8()) * scale);
        }
    }
    safetensors::write_tensor(out, name, rows, cols, &values).map_err(err)?;
    emit(&json!({
        "tensor": name,
        "rows": rows,
        "cols": cols,
        "bytes": values.len() * 4,
        "out": out,
    }))
}
