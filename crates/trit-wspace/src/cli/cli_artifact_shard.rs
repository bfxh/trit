//! CLI `split-artifact` / `join-artifact`：单层 WSP1 按行分片与合并。

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::artifact_shard;
use crate::to_wsp;

use super::{atomic_write, emit, err, load_net, one, parse, req_usize, value};

pub(super) fn split(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "split-artifact <artifact|-> --rows N --out <dir>")?;
    let rows = req_usize(&parsed, "rows")?;
    let out = value(&parsed, "out")?;
    let net = load_net(path)?;
    let (manifest, texts) = artifact_shard::split(&net, rows).map_err(err)?;
    fs::create_dir_all(out).map_err(|error| format!("{out}: {error}"))?;
    for (shard, text) in manifest.shards.iter().zip(texts) {
        let path = Path::new(out).join(&shard.file);
        fs::write(&path, text).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    let manifest_path = Path::new(out).join("manifest.json");
    let text = serde_json::to_string_pretty(&manifest.to_json())
        .map_err(|error| format!("manifest 序列化失败：{error}"))?;
    fs::write(&manifest_path, text)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    emit(&json!({
        "out": out,
        "manifest": manifest_path.display().to_string(),
        "rows": manifest.rows,
        "cols": manifest.cols,
        "shard_rows": manifest.shard_rows,
        "shards": manifest.shards.len(),
    }))
}

pub(super) fn join(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(&parsed, "join-artifact <dir> --out <path>")?;
    let out = value(&parsed, "out")?;
    let manifest_path = Path::new(dir).join("manifest.json");
    let text = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|error| format!("manifest JSON 非法：{error}"))?;
    let manifest = artifact_shard::WeightManifest::from_json(&value).map_err(err)?;
    let mut texts = Vec::with_capacity(manifest.shards.len());
    for shard in &manifest.shards {
        let path = Path::new(dir).join(&shard.file);
        texts.push(
            fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        );
    }
    let net = artifact_shard::join(&manifest, &texts).map_err(err)?;
    atomic_write(out, &to_wsp(&net))?;
    emit(&json!({
        "dir": dir,
        "out": out,
        "rows": manifest.rows,
        "cols": manifest.cols,
        "shards": manifest.shards.len(),
    }))
}
