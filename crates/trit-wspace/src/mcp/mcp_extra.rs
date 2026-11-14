//! MCP 扩展工具：tile 编辑 / 块索引 / 读块 / 写锁（从 `mcp.rs` 的 dispatch 拆出）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use trit_core::Ternary;
use trit_export::{MetaValue, Model, Tensor};

use crate::cli::atomic_write;
use crate::codec::{from_wsp, to_wsp};
use crate::edit::{Edit, Patch};
use crate::learned::{self, Scorer};
use crate::net::{Layer, Net};
use crate::ops;
use crate::trit::Trit;
use crate::wire;
use crate::wsp2::BlockIndex;

use super::load::{confined, flag, load, opt_usize, optional_oracle, required_oracle};

/// advisory 锁的 stale 阈值（与 `cli` 的写锁一致：10 分钟）。
const STALE_MS: u128 = 600_000;
const POLL_MS: u64 = 50;

pub(super) fn call(name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "wsp_tile" => tile(args),
        "wsp_index_artifact" => index_artifact(args),
        "wsp_read_block" => read_block(args),
        "wsp_lock" => lock(args),
        "wsp_train_proposer" => train_proposer(args),
        "wsp_propose_learned" => propose_learned(args),
        "wsp_propose_gradient" => propose_gradient(args),
        "wsp_export_gguf" => export_gguf(args),
        "wsp_import_gguf" => import_gguf(args),
        "wsp_quantize_delta" => quantize_delta(args),
        "wsp_certify" => certify(args),
        "wsp_task_trim" => task_trim(args),
        other => Err(format!("未知工具：{other}")),
    }
}

fn trit_of(raw: &str) -> Result<Trit, String> {
    let ch = raw
        .trim()
        .chars()
        .next()
        .ok_or_else(|| "三值不能为空".to_string())?;
    Trit::from_char(ch).ok_or_else(|| format!("非法三值字符 `{ch}`"))
}

fn tile(args: &Value) -> Result<Value, String> {
    let net = load(args)?;
    let layer = opt_usize(args, "layer", 0);
    let row_start = opt_usize(args, "row_start", 0);
    let rows = opt_usize(args, "rows", 0);
    let col_start = opt_usize(args, "col_start", 0);
    let cols = opt_usize(args, "cols", 0);
    if rows == 0 || cols == 0 {
        return Err("rows / cols 必须 > 0".into());
    }
    let edit = match (
        args.get("fill").and_then(Value::as_str),
        args.get("values").and_then(Value::as_str),
    ) {
        (Some(raw), None) => Edit::FillTile {
            layer,
            row_start,
            rows,
            col_start,
            cols,
            to: trit_of(raw)?,
        },
        (None, Some(raw)) => {
            let values: Vec<Trit> = raw
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .filter_map(Trit::from_char)
                .collect();
            if values.len() != rows * cols {
                return Err(format!(
                    "values 需要 {} 个三值，得到 {}",
                    rows * cols,
                    values.len()
                ));
            }
            Edit::SetTile {
                layer,
                row_start,
                rows,
                col_start,
                cols,
                values,
            }
        }
        _ => return Err("fill 与 values 必须且只能给一个".into()),
    };
    let patch = Patch::new(vec![edit]);
    let oracle = optional_oracle(args)?;
    let require_after = flag(args, "require_after");
    if require_after && oracle.is_none() {
        return Err("require_after 需要 oracle".into());
    }
    let outcome = ops::apply_patch(&net, patch.clone(), oracle.as_ref(), false, require_after)
        .map_err(|error| error.to_string())?;
    let work = outcome.net;
    let mut value = json!({
        "patch": wire::patch_to_value(&patch),
        "report": outcome.report,
        "applied": outcome.applied,
        "rolled_back": outcome.rolled_back,
        "summary": wire::net_summary(&work),
        "artifact": to_wsp(&work),
    });
    if let Some(oracle) = &oracle {
        value["verify"] = json!({
            "exact": oracle.exact(&work).map_err(|error| error.to_string())?,
            "loss": oracle.loss(&work).map_err(|error| error.to_string())?,
            "mismatches": wire::mismatches_value(&work, oracle, 8)
                .map_err(|error| error.to_string())?,
        });
    }
    Ok(value)
}

fn index_artifact(args: &Value) -> Result<Value, String> {
    let raw = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or("需要 source")?;
    let path = confined(raw)?;
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let index = BlockIndex::scan(&bytes).map_err(|error| error.to_string())?;
    let json_value = index.to_json();
    let text = serde_json::to_string_pretty(&json_value)
        .map_err(|error| format!("索引 JSON 序列化失败：{error}"))?;
    let mut value = json!({
        "source": raw,
        "source_bytes": bytes.len(),
        "layers": index.layers.len(),
        "blocks": index.layers.iter().map(|layer| layer.row_blocks.len()).sum::<usize>(),
        "block_rows": index.block_rows,
        "index": json_value,
    });
    if let Some(out) = args.get("out").and_then(Value::as_str) {
        let out_path = write_path(out)?;
        atomic_write(&out_path.display().to_string(), &text)?;
        value["out"] = json!(out);
    }
    Ok(value)
}

fn read_block(args: &Value) -> Result<Value, String> {
    let raw = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or("需要 source")?;
    let path = confined(raw)?;
    let index_value = args.get("index").ok_or("需要 index")?;
    let index = BlockIndex::from_json(index_value).map_err(|error| error.to_string())?;
    let layer = opt_usize(args, "layer", 0);
    let info = index
        .layers
        .get(layer)
        .ok_or_else(|| format!("层号 {layer} 越界（索引共 {} 层）", index.layers.len()))?;
    let row_start = opt_usize(args, "row_start", 0);
    if row_start > info.rows {
        return Err(format!("row_start {row_start} 超过层行数 {}", info.rows));
    }
    let rows = match args.get("rows").and_then(Value::as_u64) {
        Some(raw) => usize::try_from(raw).map_err(|_| "rows 超出 usize".to_string())?,
        None => info.rows - row_start,
    };
    let bytes = fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    let block = index
        .read_rows(&bytes, layer, row_start, rows)
        .map_err(|error| error.to_string())?;
    let net = Net::new(vec![block]).map_err(|error| error.to_string())?;
    Ok(json!({
        "layer": layer,
        "row_start": row_start,
        "rows": rows,
        "summary": wire::net_summary(&net),
        "artifact": to_wsp(&net),
    }))
}

fn lock_path(path: &Path) -> PathBuf {
    let mut raw = path.as_os_str().to_owned();
    raw.push(".lock");
    PathBuf::from(raw)
}

fn unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delay| delay.as_millis())
        .unwrap_or(0)
}

fn lock_holder(path: &Path) -> Result<Option<Value>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let pid = text
        .lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    let stamp = text
        .lines()
        .find_map(|line| line.strip_prefix("unix_ms="))
        .and_then(|raw| raw.trim().parse::<u128>().ok());
    Ok(Some(json!({
        "pid": pid,
        "age_ms": stamp.map(|stamp| unix_ms().saturating_sub(stamp)),
        "stale": stamp.is_some_and(|stamp| unix_ms().saturating_sub(stamp) > STALE_MS),
    })))
}

fn lock(args: &Value) -> Result<Value, String> {
    let raw = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or("需要 path")?;
    let path = confined(raw)?;
    let lock_file = lock_path(&path);
    match args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status")
    {
        "status" => {
            let holder = lock_holder(&lock_file)?;
            Ok(json!({
                "lock": lock_file.display().to_string(),
                "held": holder.is_some(),
                "holder": holder,
            }))
        }
        "acquire" => {
            let wait = u128::try_from(opt_usize(args, "wait_ms", 0)).unwrap_or(0);
            let pid = std::process::id();
            let started = Instant::now();
            loop {
                match fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&lock_file)
                {
                    Ok(mut file) => {
                        let _ = writeln!(file, "pid={pid}");
                        let _ = writeln!(file, "unix_ms={}", unix_ms());
                        let _ = file.sync_all();
                        return Ok(json!({
                            "lock": lock_file.display().to_string(),
                            "acquired": true,
                            "pid": pid,
                        }));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        let holder = lock_holder(&lock_file)?;
                        if holder
                            .as_ref()
                            .is_some_and(|holder| holder["stale"] == json!(true))
                        {
                            let _ = fs::remove_file(&lock_file);
                            continue;
                        }
                        if started.elapsed().as_millis() >= wait {
                            return Ok(json!({
                                "lock": lock_file.display().to_string(),
                                "acquired": false,
                                "holder": holder,
                            }));
                        }
                        std::thread::sleep(Duration::from_millis(POLL_MS));
                    }
                    Err(error) => {
                        return Err(format!("{}: {error}", lock_file.display()));
                    }
                }
            }
        }
        "release" => {
            let pid = std::process::id();
            match lock_holder(&lock_file)? {
                Some(holder) if holder["pid"] == json!(pid) => {
                    fs::remove_file(&lock_file)
                        .map_err(|error| format!("{}: {error}", lock_file.display()))?;
                    Ok(json!({
                        "lock": lock_file.display().to_string(),
                        "released": true,
                    }))
                }
                Some(holder) => Ok(json!({
                    "lock": lock_file.display().to_string(),
                    "released": false,
                    "holder": holder,
                })),
                None => Ok(json!({
                    "lock": lock_file.display().to_string(),
                    "released": false,
                    "reason": "没有锁",
                })),
            }
        }
        other => Err(format!(
            "action 只支持 status / acquire / release，得到 `{other}`"
        )),
    }
}

fn train_proposer(args: &Value) -> Result<Value, String> {
    let net = load(args)?;
    let oracle = required_oracle(args)?;
    let analysis = crate::analyze::analyze(&net, &oracle, 0).map_err(|error| error.to_string())?;
    if analysis.before_loss == 0 {
        return Err("工件已经精确，没有可学习的收益".into());
    }
    let samples = learned::samples_from_analysis(&net, &oracle, &analysis)
        .map_err(|error| error.to_string())?;
    let epochs = opt_usize(args, "epochs", 200);
    let lr = args.get("lr").and_then(Value::as_f64).unwrap_or(0.05) as f32;
    let holdout = args.get("holdout").and_then(Value::as_f64).unwrap_or(0.2) as f32;
    let seed = args.get("seed").and_then(Value::as_u64).unwrap_or(7);
    let (scorer, report) = learned::train_holdout(&samples, holdout, epochs, lr, seed);
    Ok(json!({
        "model": scorer.to_json(),
        "model_info": {
            "features": learned::FEATURE_NAMES.len(),
            "samples": scorer.samples,
            "epochs": scorer.epochs,
            "lr": scorer.lr,
        },
        "report": {
            "samples": report.samples,
            "train": report.train,
            "holdout": report.holdout,
            "epochs": report.epochs,
            "lr": report.lr,
            "train_mse": report.train_mse,
            "holdout_mse": report.holdout_mse,
            "holdout_positive": report.holdout_positive,
            "top_k": report.top_k,
            "top_k_hit": report.top_k_hit,
        },
    }))
}

fn scorer_of(model: &Value) -> Result<Scorer, String> {
    let value = match model.as_str() {
        Some(text) if text.trim_start().starts_with('{') => {
            serde_json::from_str(text).map_err(|error| format!("model JSON 非法：{error}"))?
        }
        Some(path) => {
            let path = confined(path)?;
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("{}: {error}", path.display()))?;
            serde_json::from_str(&text).map_err(|error| format!("model JSON 非法：{error}"))?
        }
        None => model.clone(),
    };
    Scorer::from_json(&value).map_err(|error| error.to_string())
}

fn score_candidates(
    net: &Net,
    oracle: &crate::oracle::Oracle,
    scored: &[learned::ScoredEdit],
    verify: bool,
    before_loss: usize,
) -> Result<Vec<Value>, String> {
    let mut candidates = Vec::with_capacity(scored.len());
    for item in scored {
        let mut value = json!({
            "edit": wire::edit_to_value(&item.edit),
            "score": item.score,
        });
        if verify {
            let mut trial = net.clone();
            item.edit
                .apply_to(&mut trial)
                .map_err(|error| error.to_string())?;
            let after_loss = oracle.loss(&trial).map_err(|error| error.to_string())?;
            value["after_loss"] = json!(after_loss);
            value["gain"] = json!(before_loss as isize - after_loss as isize);
        }
        candidates.push(value);
    }
    Ok(candidates)
}

fn propose_learned(args: &Value) -> Result<Value, String> {
    let net = load(args)?;
    let oracle = required_oracle(args)?;
    let scorer = scorer_of(args.get("model").ok_or("需要 model")?)?;
    let limit = opt_usize(args, "limit", 16);
    let evaluated = crate::repair::candidates(&net, false).len();
    let scored =
        learned::propose(&net, &oracle, &scorer, limit).map_err(|error| error.to_string())?;
    let before_loss = oracle.loss(&net).map_err(|error| error.to_string())?;
    let verify = flag(args, "verify");
    let candidates = score_candidates(&net, &oracle, &scored, verify, before_loss)?;
    Ok(json!({
        "before_loss": before_loss,
        "evaluated": evaluated,
        "limit": limit,
        "verified": verify,
        "model": {"samples": scorer.samples, "epochs": scorer.epochs, "lr": scorer.lr},
        "candidates": candidates,
    }))
}

fn propose_gradient(args: &Value) -> Result<Value, String> {
    let net = load(args)?;
    let oracle = required_oracle(args)?;
    let limit = opt_usize(args, "limit", 16);
    let before_loss = oracle.loss(&net).map_err(|error| error.to_string())?;
    let mut scored = learned::gradient_scores(&net, &oracle).map_err(|error| error.to_string())?;
    let evaluated = scored.len();
    if limit > 0 {
        scored.truncate(limit);
    }
    let verify = flag(args, "verify");
    let candidates = score_candidates(&net, &oracle, &scored, verify, before_loss)?;
    Ok(json!({
        "before_loss": before_loss,
        "evaluated": evaluated,
        "limit": limit,
        "verified": verify,
        "candidates": candidates,
    }))
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn ternary_of(value: Trit) -> Ternary {
    match value {
        Trit::Neg => Ternary::False,
        Trit::Zero => Ternary::Unknown,
        Trit::Pos => Ternary::True,
    }
}

fn trit_i8(value: Ternary) -> i8 {
    match value {
        Ternary::False => -1,
        Ternary::Unknown => 0,
        Ternary::True => 1,
    }
}

fn export_gguf(args: &Value) -> Result<Value, String> {
    let raw = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or("需要 source")?;
    let dir = confined(raw)?;
    let out = args.get("out").and_then(Value::as_str).ok_or("需要 out")?;
    let out_path = write_path(out)?;
    let manifest_path = dir.join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let manifest: Value =
        serde_json::from_str(&text).map_err(|error| format!("manifest JSON 非法：{error}"))?;
    let entries = manifest
        .get("tensors")
        .and_then(Value::as_array)
        .ok_or("manifest 缺 tensors 数组")?;
    if entries.is_empty() {
        return Err("manifest 里没有张量".into());
    }
    let mut names = BTreeSet::new();
    let mut tensors = Vec::with_capacity(entries.len());
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or("manifest 条目缺 name")?;
        if !names.insert(name.to_string()) {
            return Err(format!("manifest 里有重复张量名 `{name}`"));
        }
        let file = entry
            .get("file")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{name} 缺 file"))?;
        let path = dir.join(file);
        let text =
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let net = from_wsp(&text).map_err(|error| error.to_string())?;
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
                .map(|value| ternary_of(*value))
                .collect(),
            scales: layer.scales().to_vec(),
            bias: layer
                .bias()
                .iter()
                .map(|value| ternary_of(*value))
                .collect(),
        });
    }
    let mut metadata = BTreeMap::new();
    metadata.insert("trit.wsp1".to_string(), MetaValue::U32(1));
    let model = Model { metadata, tensors };
    let count = model.tensors.len();
    let bytes = trit_export::write(&model).map_err(|error| format!("GGUF: {error}"))?;
    fs::write(&out_path, &bytes).map_err(|error| format!("{}: {error}", out_path.display()))?;
    Ok(json!({"out": out, "bytes": bytes.len(), "tensors": count}))
}

fn import_gguf(args: &Value) -> Result<Value, String> {
    let raw = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or("需要 source")?;
    let source = confined(raw)?;
    let out = args.get("out").and_then(Value::as_str).ok_or("需要 out")?;
    let out_dir = write_path(out)?;
    let bytes = fs::read(&source).map_err(|error| format!("{}: {error}", source.display()))?;
    let model = trit_export::read(&bytes).map_err(|error| format!("GGUF: {error}"))?;
    if model.tensors.is_empty() {
        return Err(format!("{} 里没有张量", source.display()));
    }
    fs::create_dir_all(&out_dir).map_err(|error| format!("{}: {error}", out_dir.display()))?;
    let mut entries = Vec::with_capacity(model.tensors.len());
    for (index, tensor) in model.tensors.iter().enumerate() {
        let trits: Vec<i8> = tensor.trits.iter().map(|value| trit_i8(*value)).collect();
        let bias: Vec<i8> = tensor.bias.iter().map(|value| trit_i8(*value)).collect();
        let layer = Layer::with_scales(
            tensor.rows,
            tensor.cols,
            &trits,
            &bias,
            tensor.scales.clone(),
        )
        .map_err(|error| error.to_string())?;
        let net = Net::new(vec![layer]).map_err(|error| error.to_string())?;
        let file = format!("{:04}-{}.wsp", index, safe_name(&tensor.name));
        let path = out_dir.join(&file);
        fs::write(&path, to_wsp(&net)).map_err(|error| format!("{}: {error}", path.display()))?;
        entries.push(json!({
            "name": tensor.name,
            "file": file,
            "rows": tensor.rows,
            "cols": tensor.cols,
        }));
    }
    let metadata: Vec<&String> = model.metadata.keys().collect();
    let manifest = json!({"source": raw, "tensors": entries, "metadata": metadata});
    let text = serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?;
    fs::write(out_dir.join("manifest.json"), text)
        .map_err(|error| format!("manifest.json: {error}"))?;
    Ok(json!({"out": out, "tensors": entries.len(), "metadata": metadata}))
}

fn quantize_delta(args: &Value) -> Result<Value, String> {
    let base_raw = args
        .get("base")
        .and_then(Value::as_str)
        .ok_or("需要 base")?;
    let tuned_raw = args
        .get("tuned")
        .and_then(Value::as_str)
        .ok_or("需要 tuned")?;
    let tensor = args
        .get("tensor")
        .and_then(Value::as_str)
        .ok_or("需要 tensor")?;
    let trim = args.get("trim").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    if !(0.0..=1.0).contains(&trim) {
        return Err("trim 必须在 [0, 1]".into());
    }
    let base = confined(base_raw)?.display().to_string();
    let tuned = confined(tuned_raw)?.display().to_string();
    let bias_name = args
        .get("bias")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}.bias", tensor.strip_suffix(".weight").unwrap_or(tensor)));
    let base_weights =
        crate::safetensors::read_tensor(&base, tensor).map_err(|error| error.to_string())?;
    let tuned_weights =
        crate::safetensors::read_tensor(&tuned, tensor).map_err(|error| error.to_string())?;
    let base_bias = crate::safetensors::read_vector(&base, &bias_name).ok();
    let tuned_bias = crate::safetensors::read_vector(&tuned, &bias_name).ok();
    let (patch, net, stats) = crate::delta::diff_weights(
        &base_weights,
        &tuned_weights,
        base_bias.as_deref(),
        tuned_bias.as_deref(),
        trim,
    )
    .map_err(|error| error.to_string())?;
    let mut value = json!({
        "tensor": tensor,
        "bias": bias_name,
        "patch": wire::patch_to_value(&patch),
        "summary": wire::net_summary(&net),
        "artifact": to_wsp(&net),
        "stats": {
            "changed_trits": stats.changed_trits,
            "changed_bias": stats.changed_bias,
            "kept_trits": stats.kept_trits,
            "kept_bias": stats.kept_bias,
            "trim": stats.trim,
        },
    });
    if let Some(oracle) = optional_oracle(args)? {
        value["verify"] = json!({
            "loss": oracle.loss(&net).map_err(|error| error.to_string())?,
            "exact": oracle.exact(&net).map_err(|error| error.to_string())?,
            "mismatches": wire::mismatches_value(&net, &oracle, 8)
                .map_err(|error| error.to_string())?,
        });
    }
    Ok(value)
}

fn certify(args: &Value) -> Result<Value, String> {
    let net = load(args)?;
    let keep = match (args.get("keep"), args.get("keep_samples")) {
        (Some(value), None) => {
            crate::wire::oracle_from_value(value).map_err(|error| error.to_string())?
        }
        (None, Some(_)) => crate::certify::behavior_set(
            &net,
            opt_usize(args, "keep_samples", 16),
            args.get("seed").and_then(Value::as_u64).unwrap_or(7),
        )
        .map_err(|error| error.to_string())?,
        _ => return Err("keep 与 keep_samples 必须且只能给一个".into()),
    };
    let edit = args
        .get("edit")
        .map(crate::wire::oracle_from_value)
        .transpose()
        .map_err(|error| error.to_string())?;
    let options = crate::certify::CertifyOptions {
        allow: opt_usize(args, "allow", 0),
        rows: opt_usize(args, "limit", crate::certify::DEFAULT_ROWS),
    };
    let (certificate, edited) = match (args.get("patch"), args.get("edited")) {
        (Some(value), None) => {
            let patch = crate::wire::patch_from_value(value).map_err(|error| error.to_string())?;
            crate::certify::apply_and_certify(&net, &patch, &keep, edit.as_ref(), &options)
                .map_err(|error| error.to_string())?
        }
        (None, Some(raw)) => {
            let raw = raw.as_str().ok_or("edited 必须是 artifact 文本或路径")?;
            let edited = super::load::parse_or_path(raw)?;
            let certificate =
                crate::certify::certify_nets(&net, &edited, &keep, edit.as_ref(), &options)
                    .map_err(|error| error.to_string())?;
            (certificate, edited)
        }
        _ => return Err("patch 与 edited 必须且只能给一个".into()),
    };
    Ok(json!({
        "certificate": certificate.to_json(),
        "summary": wire::net_summary(&edited),
        "artifact": to_wsp(&edited),
    }))
}

fn task_trim(args: &Value) -> Result<Value, String> {
    let base = super::load::load_key(args, "base")?;
    let raw_tasks = args
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or("需要 tasks")?;
    if raw_tasks.is_empty() {
        return Err("tasks 不能为空".into());
    }
    let mut tasks = Vec::with_capacity(raw_tasks.len());
    for (index, raw) in raw_tasks.iter().enumerate() {
        let text = raw
            .as_str()
            .ok_or_else(|| format!("tasks[{index}] 必须是 artifact 文本或路径"))?;
        let tuned = super::load::parse_or_path(text)?;
        tasks.push(
            crate::task::TaskVector::between(&base, &tuned).map_err(|error| error.to_string())?,
        );
    }
    let weights: Vec<f32> = match args.get("weights").and_then(Value::as_array) {
        Some(values) => {
            if values.len() != tasks.len() {
                return Err(format!(
                    "weights 长度 {} 与 tasks 数 {} 不等",
                    values.len(),
                    tasks.len()
                ));
            }
            values
                .iter()
                .map(|value| value.as_f64().unwrap_or(1.0) as f32)
                .collect()
        }
        None => vec![1.0; tasks.len()],
    };
    let top = match (args.get("trim"), args.get("trim_fraction")) {
        (Some(count), None) => {
            let count = count.as_u64().ok_or("trim 需要非负整数")?;
            let count = usize::try_from(count).map_err(|_| "trim 超出 usize".to_string())?;
            Some(crate::task_merge::TrimTop::Count(count))
        }
        (None, Some(ratio)) => {
            let ratio = ratio.as_f64().ok_or("trim_fraction 需要数字")?;
            Some(crate::task_merge::TrimTop::Fraction(ratio as f32))
        }
        (None, None) => None,
        _ => return Err("trim 与 trim_fraction 只能给一个".into()),
    };
    let trim = crate::task_merge::TrimOptions {
        top,
        margin: args
            .get("trim_margin")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
    };
    let (merged, report) =
        crate::task_merge::merge_tasks_weighted_trim_report(&base, &tasks, &weights, &trim)
            .map_err(|error| error.to_string())?;
    Ok(json!({
        "artifact": to_wsp(&merged),
        "summary": wire::net_summary(&merged),
        "merge_report": report.to_json(),
    }))
}

/// 写路径校验：父目录必须存在且在工作区内，目标本身可以还不存在。
fn write_path(raw: &str) -> Result<PathBuf, String> {
    let root = std::env::current_dir()
        .map_err(|_| "无法确定工作区根".to_string())?
        .canonicalize()
        .map_err(|_| "工作区根不可用".to_string())?;
    let path = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        root.join(raw)
    };
    let parent = path
        .parent()
        .ok_or_else(|| "路径没有父目录".to_string())?
        .canonicalize()
        .map_err(|_| format!("{raw}: 父目录不可用"))?;
    if !parent.starts_with(&root) {
        return Err(format!("{raw}: 越界（不在工作区内）"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| "路径没有文件名".to_string())?;
    Ok(parent.join(name))
}
