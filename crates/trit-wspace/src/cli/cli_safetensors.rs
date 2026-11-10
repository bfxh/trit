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
        "import-safetensors <file> --tensor <name>[,<name>...] [--out <path>]",
    )?;
    let raw = value(&parsed, "tensor")?;
    let names: Vec<&str> = raw
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    if names.is_empty() {
        return Err("--tensor 不能为空".into());
    }
    let mut layers = Vec::with_capacity(names.len());
    let mut report = Vec::with_capacity(names.len());
    for name in &names {
        let weights = safetensors::read_tensor(path, name).map_err(err)?;
        report.push(json!({
            "name": name,
            "rows": weights.rows,
            "cols": weights.cols,
            "values": weights.values.len(),
        }));
        layers.push(weights.to_layer().map_err(err)?);
    }
    let net = crate::Net::new(layers).map_err(err)?;
    write_out(&parsed, &net)?;
    let mut value = json!({
        "tensors": report,
        "summary": crate::wire::net_summary(&net),
        "wsp1": to_wsp(&net),
    });
    if report.len() == 1 {
        value["tensor"] = report[0].clone();
    }
    emit(&value)
}

pub(super) fn export(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "export-safetensors <artifact|-> --out <path> [--tensor <name>[,<name>...]]",
    )?;
    let out = value(&parsed, "out")?;
    let net = load_net(path)?;
    let layers = net.layers();
    let names: Vec<String> = match parsed.values.get("tensor") {
        Some(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        None => (0..layers.len())
            .map(|index| format!("layer{index}"))
            .collect(),
    };
    if names.len() != layers.len() {
        return Err(format!(
            "--tensor 给了 {} 个名字，网络有 {} 层",
            names.len(),
            layers.len()
        ));
    }
    let mut buffered: Vec<(usize, usize, Vec<f32>)> = Vec::with_capacity(layers.len());
    for layer in layers {
        let (rows, cols) = (layer.rows(), layer.cols());
        let mut values = Vec::with_capacity(rows * cols);
        for row in 0..rows {
            let scale = layer.scale(row).expect("行在范围内");
            for col in 0..cols {
                let trit = layer.trit(row, col).expect("列在范围内");
                values.push(f32::from(trit.to_i8()) * scale);
            }
        }
        buffered.push((rows, cols, values));
    }
    let tensors: Vec<safetensors::TensorOut> = names
        .iter()
        .zip(&buffered)
        .map(|(name, (rows, cols, values))| safetensors::TensorOut {
            name,
            rows: *rows,
            cols: *cols,
            values,
        })
        .collect();
    safetensors::write_tensors(out, &tensors).map_err(err)?;
    let bytes: usize = buffered.iter().map(|(_, _, values)| values.len() * 4).sum();
    emit(&json!({
        "tensors": names,
        "layers": layers.len(),
        "bytes": bytes,
        "out": out,
    }))
}
