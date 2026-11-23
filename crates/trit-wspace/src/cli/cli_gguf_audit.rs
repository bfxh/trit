//! `gguf-audit`：模型级 GGUF 往返复验，区分语义核心与已知的字节/标记损失。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::{Net, from_wsp};
use serde_json::{Value, json};
use trit_export::Model;

use super::super::{emit, one, parse};
use super::io::{read_source, write_model_dir};

/// 语义核心：trits、三值 bias、scale 位模式；f32 bias 幅度不属于行为判据。
fn tensor_core_equal(left: &Net, right: &Net) -> bool {
    let (a, b) = match (left.layers().first(), right.layers().first()) {
        (Some(a), Some(b)) if left.layers().len() == 1 && right.layers().len() == 1 => (a, b),
        _ => return false,
    };
    a.rows() == b.rows()
        && a.cols() == b.cols()
        && a.trits() == b.trits()
        && a.bias() == b.bias()
        && a.scales()
            .iter()
            .zip(b.scales())
            .all(|(x, y)| x.to_bits() == y.to_bits())
}

fn dir_bytes(dir: &Path) -> u64 {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .filter(|meta| meta.is_file())
                .map(|meta| meta.len())
                .sum()
        })
        .unwrap_or(0)
}

fn back_files(manifest: &Value) -> Result<BTreeMap<String, (String, bool, bool)>, String> {
    let entries = manifest
        .get("tensors")
        .and_then(Value::as_array)
        .ok_or_else(|| "回读 manifest 缺 tensors".to_string())?;
    let mut out = BTreeMap::new();
    for entry in entries {
        let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
        let file = entry.get("file").and_then(Value::as_str).unwrap_or("");
        out.insert(
            name.to_string(),
            (
                file.to_string(),
                entry
                    .get("vector")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                entry.get("bias").is_some(),
            ),
        );
    }
    Ok(out)
}

pub(crate) fn audit(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(&parsed, "gguf-audit <dir> [--out <report.json>]")?;
    let report_out = parsed.values.get("out");
    let (model, source) = read_source(dir)?;
    let gguf_bytes = trit_export::write(&model).map_err(|error| format!("GGUF: {error}"))?;
    let trit_bytes: usize = model
        .tensors
        .iter()
        .map(|tensor| trit_export::tensor_size(tensor.rows, tensor.cols))
        .sum();

    let temp: PathBuf =
        std::env::temp_dir().join(format!("trit-wspace-gguf-audit-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp);
    let back_model =
        trit_export::read(&gguf_bytes).map_err(|error| format!("GGUF 回读: {error}"))?;
    write_model_dir(&back_model, &temp)?;
    let back_manifest_text = fs::read_to_string(temp.join("manifest.json"))
        .map_err(|error| format!("{}: {error}", temp.join("manifest.json").display()))?;
    let back_manifest: Value = serde_json::from_str(&back_manifest_text)
        .map_err(|error| format!("回读 manifest JSON: {error}"))?;
    let back = back_files(&back_manifest)?;

    let mut byte_identical = 0usize;
    let mut semantics_equal = 0usize;
    let mut biasf_lost = 0usize;
    let mut other_mismatch = Vec::new();
    let mut biasf_mismatch = Vec::new();
    let mut vector_markers = 0usize;
    let mut vector_preserved = 0usize;
    let mut bias_markers = 0usize;
    let mut bias_preserved = 0usize;
    let mut matched = 0usize;
    for entry in &source {
        let original_path = Path::new(dir).join(&entry.file);
        let original_text = fs::read_to_string(&original_path)
            .map_err(|error| format!("{}: {error}", original_path.display()))?;
        let original = from_wsp(&original_text).map_err(|error| error.to_string())?;
        let Some((file, has_vector, has_bias)) = back.get(&entry.name) else {
            other_mismatch
                .push(json!({"name": entry.name, "reasons": ["missing_after_roundtrip"]}));
            continue;
        };
        matched += 1;
        let back_path = temp.join(file);
        let back_text = fs::read_to_string(&back_path)
            .map_err(|error| format!("{}: {error}", back_path.display()))?;
        let restored = from_wsp(&back_text).map_err(|error| error.to_string())?;
        if original_text == back_text {
            byte_identical += 1;
        }
        let core = tensor_core_equal(&original, &restored);
        if core {
            semantics_equal += 1;
        }
        let biasf = original.layers()[0].bias_f32().len() == original.layers()[0].rows();
        let biasf_gone = biasf && restored.layers()[0].bias_f32().is_empty();
        if biasf_gone {
            biasf_lost += 1;
            if original_text != back_text {
                biasf_mismatch.push(json!({"name": entry.name}));
            }
        }
        if entry.vector {
            vector_markers += 1;
            if *has_vector {
                vector_preserved += 1;
            }
        }
        if entry.bias.is_some() {
            bias_markers += 1;
            if *has_bias {
                bias_preserved += 1;
            }
        }
        if original_text != back_text && !biasf_gone {
            other_mismatch.push(json!({
                "name": entry.name,
                "semantic_equal": core,
            }));
        }
        if !core {
            other_mismatch.push(json!({"name": entry.name, "reasons": ["semantic_core"]}));
        }
    }

    let count_ok = source.len() == back.len() && matched == source.len();
    let metadata_equal = metadata_equal(&model, &back_model);
    let ok = count_ok && metadata_equal && semantics_equal == source.len();
    let report = json!({
        "schema": 1,
        "kind": "trit-wspace-gguf-audit",
        "source": dir,
        "tensors": source.len(),
        "roundtrip_tensors": back.len(),
        "count_equal": count_ok,
        "metadata_equal": metadata_equal,
        "gguf_bytes": gguf_bytes.len(),
        "trit_bytes": trit_bytes,
        "source_wsp_bytes": dir_bytes(Path::new(dir)),
        "roundtrip_wsp_bytes": dir_bytes(&temp),
        "bytes_identical": byte_identical,
        "semantics_equal": semantics_equal,
        "biasf_lost": biasf_lost,
        "manifest": {
            "vector_markers": vector_markers,
            "vector_preserved": vector_preserved,
            "bias_markers": bias_markers,
            "bias_preserved": bias_preserved,
        },
        "biasf_mismatches": biasf_mismatch,
        "other_mismatches": other_mismatch,
        "ok": ok,
    });
    if let Some(path) = report_out {
        let text =
            serde_json::to_string_pretty(&report).map_err(|error| format!("JSON: {error}"))?;
        super::super::fsutil::atomic_write(path, &text)?;
    }
    let _ = fs::remove_dir_all(&temp);
    emit(&report)?;
    if ok {
        Ok(())
    } else {
        Err("GGUF 模型级审计：语义核心/张量数/元数据不一致".to_string())
    }
}

fn metadata_equal(left: &Model, right: &Model) -> bool {
    left.metadata == right.metadata
}
