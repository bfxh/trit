//! CLI `import-gguf` / `export-gguf`：三值 GGUF（`trit-export`）与 WSP1 目录互转。
//!
//! 目录约定与 `import-safetensors-dir` 一致：`manifest.json` + `NNNN-<name>.wsp`，
//! 每个 `.wsp` 是单层 WSP1。GGUF 侧只接受 `TYPE_TRIT` 2D 张量；元数据不进 WSP1
//! （往返后只剩导出时写入的 `trit.wsp1` 版本标记）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use trit_core::Ternary;
use trit_export::{MetaValue, Model, Tensor};

use crate::trit::Trit;
use crate::{Layer, Net, from_wsp, to_wsp};

use super::cli_safetensors::safe_name;
use super::{emit, err, one, parse, value};

/// `Ternary`（逻辑值）→ 权重 `i8`：`False/Unknown/True → -1/0/+1`。
fn to_i8(value: Ternary) -> i8 {
    match value {
        Ternary::False => -1,
        Ternary::Unknown => 0,
        Ternary::True => 1,
    }
}

/// 权重 `Trit` → `Ternary`。
fn from_trit(value: Trit) -> Ternary {
    match value {
        Trit::Neg => Ternary::False,
        Trit::Zero => Ternary::Unknown,
        Trit::Pos => Ternary::True,
    }
}

pub(super) fn import(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let source = one(&parsed, "import-gguf <file> --out <dir>")?;
    let out = value(&parsed, "out")?;
    let bytes = fs::read(source).map_err(|error| format!("{source}: {error}"))?;
    let model = trit_export::read(&bytes).map_err(|error| format!("GGUF {source}: {error}"))?;
    if model.tensors.is_empty() {
        return Err(format!("{source} 里没有张量"));
    }
    fs::create_dir_all(out).map_err(|error| format!("{out}: {error}"))?;
    let mut entries = Vec::with_capacity(model.tensors.len());
    for tensor in &model.tensors {
        let trits: Vec<i8> = tensor.trits.iter().map(|value| to_i8(*value)).collect();
        let bias: Vec<i8> = tensor.bias.iter().map(|value| to_i8(*value)).collect();
        let layer = Layer::with_scales(
            tensor.rows,
            tensor.cols,
            &trits,
            &bias,
            tensor.scales.clone(),
        )
        .map_err(err)?;
        let net = Net::new(vec![layer]).map_err(err)?;
        let file = format!("{:04}-{}.wsp", entries.len(), safe_name(&tensor.name));
        let path = Path::new(out).join(&file);
        fs::write(&path, to_wsp(&net)).map_err(|error| format!("{}: {error}", path.display()))?;
        entries.push(json!({
            "name": tensor.name,
            "file": file,
            "rows": tensor.rows,
            "cols": tensor.cols,
        }));
    }
    let metadata: Vec<&String> = model.metadata.keys().collect();
    let manifest = json!({
        "source": source,
        "tensors": entries,
        "metadata": metadata,
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    let manifest_path = Path::new(out).join("manifest.json");
    fs::write(&manifest_path, text)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    emit(&json!({
        "out": out,
        "manifest": manifest_path.display().to_string(),
        "tensors": manifest["tensors"],
        "metadata": manifest["metadata"],
    }))
}

pub(super) fn export(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(&parsed, "export-gguf <dir> --out <file>")?;
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
    let mut names = BTreeSet::new();
    let mut tensors = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "manifest 条目缺 name".to_string())?;
        if !names.insert(name.to_string()) {
            return Err(format!("manifest 里有重复张量名 `{name}`"));
        }
        let file = entry
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} 缺 file"))?;
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
        tensors.push(Tensor {
            name: name.to_string(),
            rows: layer.rows(),
            cols: layer.cols(),
            trits: layer
                .trits()
                .iter()
                .map(|value| from_trit(*value))
                .collect(),
            scales: layer.scales().to_vec(),
            bias: layer.bias().iter().map(|value| from_trit(*value)).collect(),
        });
    }
    let mut metadata = BTreeMap::new();
    metadata.insert("trit.wsp1".to_string(), MetaValue::U32(1));
    let model = Model { metadata, tensors };
    let bytes = trit_export::write(&model).map_err(|error| format!("GGUF: {error}"))?;
    super::fsutil::atomic_write_bytes(out, &bytes)?;
    let trit_bytes: usize = model
        .tensors
        .iter()
        .map(|tensor| trit_export::tensor_size(tensor.rows, tensor.cols))
        .sum();
    emit(&json!({
        "dir": dir,
        "out": out,
        "tensors": names.iter().collect::<Vec<_>>(),
        "bytes": bytes.len(),
        "trit_bytes": trit_bytes,
    }))
}
