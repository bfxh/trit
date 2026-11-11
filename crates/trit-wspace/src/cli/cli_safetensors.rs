//! CLI `list-safetensors` / `import-safetensors` / `export-safetensors`：
//! 真实权重张量 ↔ 三值 WSP1（从 `cli.rs` 拆出，控制单文件行数）。

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::safetensors;
use crate::{from_wsp, to_wsp};

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
                "dtype": tensor.dtype,
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
        "import-safetensors <file> --tensor <name>[,<name>...] [--bias <name>] [--out <path>]",
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
    let bias_name = parsed.values.get("bias").map(String::as_str);
    if bias_name.is_some() && names.len() != 1 {
        return Err("--bias 只支持单个 --tensor".into());
    }
    let bias_values = match bias_name {
        Some(name) => Some(safetensors::read_vector(path, name).map_err(err)?),
        None => None,
    };
    let mut layers = Vec::with_capacity(names.len());
    let mut report = Vec::with_capacity(names.len());
    for name in &names {
        let (weights, vector) = safetensors::read_weights(path, name).map_err(err)?;
        report.push(json!({
            "name": name,
            "rows": weights.rows,
            "cols": weights.cols,
            "values": weights.values.len(),
            "vector": vector,
            "bias": bias_name,
        }));
        layers.push(
            weights
                .to_layer_with_bias(bias_values.as_deref())
                .map_err(err)?,
        );
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
    let parsed = parse(args, &["vector", "bias"])?;
    let path = one(
        &parsed,
        "export-safetensors <artifact|-> --out <path> [--tensor <name>[,<name>...]] [--vector] [--bias]",
    )?;
    let vector = parsed.switches.contains("vector");
    let with_bias = parsed.switches.contains("bias");
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
    if vector && (layers.len() != 1 || layers[0].rows() != 1) {
        return Err("--vector 只支持单层且行数为 1 的工件".into());
    }
    if with_bias && layers.len() != 1 {
        return Err("--bias 只支持单层导出".into());
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
    let bias_name = if with_bias {
        let name = &names[0];
        Some(
            name.strip_suffix(".weight")
                .map_or_else(|| format!("{name}.bias"), |stem| format!("{stem}.bias")),
        )
    } else {
        None
    };
    let mut bias_values: Vec<f32> = Vec::new();
    if with_bias {
        let layer = &layers[0];
        bias_values = if layer.bias_f32().len() == layer.rows() {
            layer.bias_f32().to_vec()
        } else {
            layer
                .bias()
                .iter()
                .map(|trit| f32::from(trit.to_i8()))
                .collect()
        };
    }
    let mut tensors: Vec<safetensors::TensorOut> = names
        .iter()
        .zip(&buffered)
        .map(|(name, (rows, cols, values))| safetensors::TensorOut {
            name,
            rows: *rows,
            cols: *cols,
            vector,
            values,
        })
        .collect();
    if let Some(name) = &bias_name {
        tensors.push(safetensors::TensorOut {
            name,
            rows: 1,
            cols: layers[0].rows(),
            vector: true,
            values: &bias_values,
        });
    }
    safetensors::write_tensors(out, &tensors).map_err(err)?;
    let bytes: usize = buffered
        .iter()
        .map(|(_, _, values)| values.len() * 4)
        .sum::<usize>()
        + bias_values.len() * 4;
    emit(&json!({
        "tensors": names,
        "bias": bias_name,
        "layers": layers.len(),
        "bytes": bytes,
        "out": out,
    }))
}

fn safe_name(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = mapped.trim_matches('.').trim_matches('_');
    if trimmed.is_empty() {
        "tensor".to_string()
    } else {
        trimmed.to_string()
    }
}

pub(super) fn import_dir(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let source = one(&parsed, "import-safetensors-dir <file> --out <dir>")?;
    let out = value(&parsed, "out")?;
    let tensors = safetensors::list(source).map_err(err)?;
    let names: BTreeSet<&str> = tensors.iter().map(|info| info.name.as_str()).collect();
    fs::create_dir_all(out).map_err(|error| format!("{out}: {error}"))?;
    let mut manifest = Vec::new();
    let mut skipped = Vec::new();
    for info in &tensors {
        if info.shape.len() > 2 {
            skipped.push(json!({
                "name": info.name,
                "reason": format!("{} 维", info.shape.len()),
            }));
            continue;
        }
        if !matches!(info.dtype.as_str(), "F32" | "F16" | "BF16") {
            skipped.push(json!({
                "name": info.name,
                "reason": format!("dtype {}", info.dtype),
            }));
            continue;
        }
        let (weights, vector) = safetensors::read_weights(source, &info.name).map_err(err)?;
        let mut bias_name = info.name.strip_suffix(".weight").and_then(|stem| {
            let candidate = format!("{stem}.bias");
            names.contains(candidate.as_str()).then_some(candidate)
        });
        let bias_values = match bias_name.as_ref() {
            Some(name) => safetensors::read_vector(source, name)
                .ok()
                .filter(|values| values.len() == weights.rows),
            None => None,
        };
        if bias_values.is_none() {
            bias_name = None;
        }
        let layer = weights
            .to_layer_with_bias(bias_values.as_deref())
            .map_err(err)?;
        let net = crate::Net::new(vec![layer]).map_err(err)?;
        let file = format!("{:04}-{}.wsp", manifest.len(), safe_name(&info.name));
        let path = Path::new(out).join(&file);
        fs::write(&path, to_wsp(&net)).map_err(|error| format!("{}: {error}", path.display()))?;
        manifest.push(json!({
            "name": info.name,
            "file": file,
            "rows": weights.rows,
            "cols": weights.cols,
            "vector": vector,
            "bias": bias_name,
        }));
    }
    if manifest.is_empty() {
        return Err(format!(
            "{source} 里没有可导入的 2D 浮点张量（跳过 {} 个）",
            skipped.len()
        ));
    }
    let manifest = json!({ "source": source, "tensors": manifest, "skipped": skipped });
    let text = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    let path = Path::new(out).join("manifest.json");
    fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    emit(&json!({
        "out": out,
        "manifest": path.display().to_string(),
        "tensors": manifest["tensors"],
        "skipped": manifest["skipped"],
    }))
}

struct BufferedLayer {
    rows: usize,
    cols: usize,
    vector: bool,
    values: Vec<f32>,
    bias_name: Option<String>,
    bias_values: Vec<f32>,
}

pub(super) fn export_dir(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(&parsed, "export-safetensors-dir <dir> --out <file>")?;
    let out = value(&parsed, "out")?;
    let manifest_path = Path::new(dir).join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: Value =
        serde_json::from_str(&text).map_err(|error| format!("manifest JSON 非法：{error}"))?;
    let entries = manifest
        .get("tensors")
        .and_then(Value::as_array)
        .ok_or_else(|| "manifest 缺 tensors 数组".to_string())?;
    if entries.is_empty() {
        return Err("manifest 里没有张量".into());
    }
    let mut names: Vec<String> = Vec::with_capacity(entries.len());
    let mut buffered: Vec<BufferedLayer> = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "manifest 条目缺 name".to_string())?;
        let file = entry
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} 缺 file"))?;
        let vector = entry
            .get("vector")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let bias_name = entry
            .get("bias")
            .and_then(Value::as_str)
            .map(str::to_string);
        let path = Path::new(dir).join(file);
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let net = from_wsp(&text).map_err(err)?;
        if net.layers().len() != 1 {
            return Err(format!(
                "{file} 是 {} 层，不是单层 WSP1",
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
        let bias_values = if bias_name.is_some() {
            if layer.bias_f32().len() == rows {
                layer.bias_f32().to_vec()
            } else {
                layer
                    .bias()
                    .iter()
                    .map(|trit| f32::from(trit.to_i8()))
                    .collect()
            }
        } else {
            Vec::new()
        };
        names.push(name.to_string());
        buffered.push(BufferedLayer {
            rows,
            cols,
            vector,
            values,
            bias_name,
            bias_values,
        });
    }
    let unique: BTreeSet<&String> = names.iter().collect();
    if unique.len() != names.len() {
        return Err("manifest 里有重复张量名".into());
    }
    let mut tensors: Vec<safetensors::TensorOut> = Vec::new();
    for (name, layer) in names.iter().zip(&buffered) {
        tensors.push(safetensors::TensorOut {
            name,
            rows: layer.rows,
            cols: layer.cols,
            vector: layer.vector,
            values: &layer.values,
        });
        if let Some(bias) = &layer.bias_name {
            tensors.push(safetensors::TensorOut {
                name: bias,
                rows: 1,
                cols: layer.rows,
                vector: true,
                values: &layer.bias_values,
            });
        }
    }
    safetensors::write_tensors(out, &tensors).map_err(err)?;
    let bytes: usize = buffered
        .iter()
        .map(|layer| layer.values.len() * 4 + layer.bias_values.len() * 4)
        .sum();
    emit(&json!({
        "dir": dir,
        "out": out,
        "tensors": names,
        "bytes": bytes,
    }))
}
