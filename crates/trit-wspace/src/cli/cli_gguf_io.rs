//! GGUF ↔ WSP1 的目录读写小件：manifest 解析、WSP1 读写、模型/目录互转。
//!
//! 与 `cli_gguf.rs` 的 CLI 壳分开，方便 `gguf-audit` 复用同一套往返路径。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use trit_core::Ternary;
use trit_export::{MetaValue, Model, Tensor};

use crate::trit::Trit;
use crate::{Layer, Net, from_wsp, to_wsp};

use super::super::cli_safetensors::safe_name;
use super::super::err;

pub(super) struct SourceEntry {
    pub name: String,
    pub file: String,
    pub vector: bool,
    pub bias: Option<String>,
}
fn to_i8(value: Ternary) -> i8 {
    match value {
        Ternary::False => -1,
        Ternary::Unknown => 0,
        Ternary::True => 1,
    }
}
fn from_trit(value: Trit) -> Ternary {
    match value {
        Trit::Neg => Ternary::False,
        Trit::Zero => Ternary::Unknown,
        Trit::Pos => Ternary::True,
    }
}
/// 读 WSP1 目录，构造 GGUF 模型，同时保留 manifest 标记供审计比较。
pub(super) fn read_source(dir: &str) -> Result<(Model, Vec<SourceEntry>), String> {
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
    let mut source = Vec::with_capacity(entries.len());
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
        let path = super::super::safe_path::resolve(dir, file).map_err(err)?;
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
        source.push(SourceEntry {
            name: name.to_string(),
            file: file.to_string(),
            vector: entry
                .get("vector")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            bias: entry
                .get("bias")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
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
    Ok((Model { metadata, tensors }, source))
}

/// 把 GGUF 模型逐张量写成单层 WSP1 目录。
pub(super) fn write_model_dir(model: &Model, out: &Path) -> Result<Vec<Value>, String> {
    fs::create_dir_all(out).map_err(|error| format!("{}: {error}", out.display()))?;
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
        let path = out.join(&file);
        fs::write(&path, to_wsp(&net)).map_err(|error| format!("{}: {error}", path.display()))?;
        entries.push(json!({
            "name": tensor.name,
            "file": file,
            "rows": tensor.rows,
            "cols": tensor.cols,
        }));
    }
    let manifest = json!({
        "source": "cli-gguf",
        "tensors": entries,
        "metadata": model.metadata.keys().collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    fs::write(out.join("manifest.json"), text)
        .map_err(|error| format!("{}: {error}", out.join("manifest.json").display()))?;
    Ok(entries)
}
