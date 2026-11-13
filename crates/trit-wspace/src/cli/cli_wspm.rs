//! CLI `pack-model` / `unpack-model`：WSP1 模型目录 ↔ WSPM 批量容器。
//!
//! 目录约定与 `import-safetensors-dir` / `import-gguf` 一致：`manifest.json` +
//! `NNNN-<name>.wsp`（单层 WSP1）。WSPM 逐张量存 WSP2/3/4 blob（默认 `auto` 逐个
//! 选最小），保留 `biasf` 与清单的 `vector` / `bias` 语义，unpack 逐字节还原。

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::wspm::{self, ModelItem};

use super::cli_safetensors::safe_name;
use super::{atomic_write_bytes, emit, err, one, parse, value};

pub(super) fn pack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(
        &parsed,
        "pack-model <dir> --out <file> [--format auto|wsp2|wsp3|wsp4]",
    )?;
    let out = value(&parsed, "out")?;
    let format = parsed
        .values
        .get("format")
        .map(String::as_str)
        .unwrap_or("auto");
    if !wspm::FORMATS.contains(&format) {
        return Err(format!(
            "--format 需要 auto|wsp2|wsp3|wsp4，得到 `{format}`"
        ));
    }
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
    let mut names = Vec::with_capacity(entries.len());
    let mut nets = Vec::with_capacity(entries.len());
    let mut source_bytes = 0u64;
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "manifest 条目缺 name".to_string())?;
        if names.iter().any(|other: &String| other == name) {
            return Err(format!("manifest 里有重复张量名 `{name}`"));
        }
        names.push(name.to_string());
    }
    for entry in entries {
        let file = entry
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| "manifest 条目缺 file".to_string())?;
        let path = super::safe_path::resolve(dir, file).map_err(err)?;
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        source_bytes += text.len() as u64;
        let net = crate::from_wsp(&text).map_err(err)?;
        if net.layers().len() != 1 {
            return Err(format!("`{file}` 不是单层 WSP1"));
        }
        nets.push(net);
    }
    let items: Vec<ModelItem<'_>> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| ModelItem {
            name: &names[index],
            vector: entry
                .get("vector")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            bias: entry.get("bias").and_then(Value::as_str),
            net: &nets[index],
        })
        .collect();
    let packed = wspm::pack(&items, format).map_err(err)?;
    let count = packed.bytes.len();
    atomic_write_bytes(out, &packed.bytes)?;
    let mut formats = std::collections::BTreeMap::new();
    for name in &packed.formats {
        *formats.entry(name.to_string()).or_insert(0usize) += 1;
    }
    emit(&json!({
        "out": out,
        "bytes": count,
        "source_bytes": source_bytes,
        "tensors": entries.len(),
        "formats": formats,
    }))
}

pub(super) fn unpack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let source = one(&parsed, "unpack-model <file> --out <dir>")?;
    let out = value(&parsed, "out")?;
    let bytes = fs::read(source).map_err(|error| format!("{source}: {error}"))?;
    let model = wspm::unpack(&bytes).map_err(err)?;
    if model.entries.is_empty() {
        return Err(format!("{source} 里没有张量"));
    }
    fs::create_dir_all(out).map_err(|error| format!("{out}: {error}"))?;
    let mut entries = Vec::with_capacity(model.entries.len());
    for (index, (entry, net)) in model.entries.iter().zip(&model.nets).enumerate() {
        let file = format!("{index:04}-{}.wsp", safe_name(&entry.name));
        let path = Path::new(out).join(&file);
        fs::write(&path, crate::to_wsp(net))
            .map_err(|error| format!("{}: {error}", path.display()))?;
        entries.push(json!({
            "name": entry.name,
            "file": file,
            "rows": entry.rows,
            "cols": entry.cols,
            "vector": entry.vector,
            "bias": entry.bias.map(|target| model.entries[target].name.clone()),
        }));
    }
    let manifest = json!({
        "source": source,
        "tensors": entries,
        "skipped": [],
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    let manifest_path = Path::new(out).join("manifest.json");
    fs::write(&manifest_path, text)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    emit(&json!({
        "out": out,
        "manifest": manifest_path.display().to_string(),
        "tensors": manifest["tensors"],
    }))
}
