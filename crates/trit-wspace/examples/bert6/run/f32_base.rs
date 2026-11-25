//! f32 基线缓存：同一模型 + 同一数据目录的 f32 STS 可跨趟复用。
//!
//! 只缓存 `f32_sts`（含 `TRIT_DUMP_PREDS=1` 时的 preds），不缓存 271 MB 的
//! f32 输出；命中时 `f32_vs_reference` / `real_vs_f32` / random 诊断记 null。
//! 只影响诊断，不影响量化模型的 STS。

use serde_json::{Value, json};

use super::data::keep_layers;
use crate::activation::ActivationMode;
use crate::model::{Bert, BertConfig};
use crate::sts::Pair;

pub(crate) struct Base {
    pub(crate) out: Vec<f32>,
    pub(crate) sts: Value,
    pub(crate) layers: Vec<Vec<f32>>,
    pub(crate) bert: Option<Bert>,
}

fn cache_path() -> Option<String> {
    std::env::var("TRIT_F32_CACHE")
        .ok()
        .filter(|p| !p.is_empty())
}

fn key(file: &str, data_dir: &str, hidden: usize, batch: usize, seq: usize) -> Value {
    json!({
        "schema": 1,
        "model": file,
        "data_dir": data_dir,
        "hidden": hidden,
        "batch": batch,
        "seq": seq,
        "layers": std::env::var("TRIT_LAYERS").unwrap_or_else(|_| "6".to_string()),
        "preds": std::env::var("TRIT_DUMP_PREDS").is_ok_and(|v| v.trim() == "1"),
    })
}
fn load(file: &str, data_dir: &str, hidden: usize, batch: usize, seq: usize) -> Option<Value> {
    let path = cache_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    if value.get("key")? != &key(file, data_dir, hidden, batch, seq) {
        return None;
    }
    value.get("f32_sts").cloned()
}

fn save(sts: &Value, file: &str, data_dir: &str, hidden: usize, batch: usize, seq: usize) {
    let Some(path) = cache_path() else {
        return;
    };
    let value = json!({
        "key": key(file, data_dir, hidden, batch, seq),
        "f32_sts": sts,
    });
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        std::fs::write(path, text + "\n").ok();
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    file: &str,
    data_dir: &str,
    input: &[f32],
    mask: &[u8],
    batch: usize,
    seq: usize,
    hidden: usize,
    eps: f32,
    pairs: &[Pair],
    allow_cache: bool,
) -> Base {
    if allow_cache && let Some(sts) = load(file, data_dir, hidden, batch, seq) {
        return Base {
            out: Vec::new(),
            sts,
            layers: Vec::new(),
            bert: None,
        };
    }
    let bert = Bert::load(
        file,
        BertConfig::new(hidden, eps, false, ActivationMode::F32),
    );
    let (out, _, layers) = bert.forward(input, mask, batch, seq, keep_layers());
    let sts = super::embed_dump::pure_sts(&out, mask, batch, seq, hidden, pairs);
    if allow_cache {
        save(&sts, file, data_dir, hidden, batch, seq);
    }
    Base {
        out,
        sts,
        layers,
        bert: Some(bert),
    }
}
