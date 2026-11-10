//! 审计/分片相关的 CLI 命令（从 `cli` 拆出，避免单文件超长）。

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::audit::AuditLog;
use crate::cli::{
    atomic_write, emit, err, load_net, no_double_stdin, one, opt_usize, parse, read_source, value,
    write_out,
};
use crate::codec::to_wsp;
use crate::manifest::{ShardManifest, text_digest};
use crate::shard::{ShardCursor, ShardReplay};
use crate::wire;

pub(crate) fn cmd_audit(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let log_path = one(&parsed, "audit <log|-> [--artifact <initial|->]")?;
    let artifact = parsed.values.get("artifact").map(String::as_str);
    let mut sources = vec![log_path];
    if let Some(artifact) = artifact {
        sources.push(artifact);
    }
    no_double_stdin(&sources)?;
    let log = AuditLog::from_jsonl(&read_source(log_path)?).map_err(err)?;
    log.verify_chain().map_err(err)?;
    let mut output = json!({
        "valid": true,
        "entries": log.len(),
        "head": format!("{:016x}", log.head()),
    });
    if let Some(artifact) = artifact {
        let replay = log.replay(&load_net(artifact)?).map_err(err)?;
        output["replay"] = json!({
            "entries": replay.entries,
            "applied": replay.applied,
            "rolled_back": replay.rolled_back,
            "skipped": replay.skipped,
            "behavior_checked": replay.behavior_checked,
            "final_checksum": format!("{:016x}", replay.final_checksum),
        });
    }
    emit(&output)
}

pub(crate) fn cmd_shard(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let log_path = one(&parsed, "shard <log|-> --size N --out <dir>")?;
    let size: usize = value(&parsed, "size")?
        .parse()
        .map_err(|_| "size 需要正整数".to_string())?;
    let out_dir = value(&parsed, "out")?;
    let log = AuditLog::from_jsonl(&read_source(log_path)?).map_err(err)?;
    let shards = log.into_shards(size).map_err(err)?;
    shards.verify_chain().map_err(err)?;
    let texts = shards.to_texts();
    fs::create_dir_all(out_dir).map_err(|error| format!("{out_dir}: {error}"))?;
    for (index, text) in texts.iter().enumerate() {
        let path = Path::new(out_dir).join(format!("shard-{index:03}.jsonl"));
        let path = path.to_str().ok_or("输出路径不是 UTF-8")?;
        atomic_write(path, text)?;
    }
    let mut manifest = ShardManifest::of(&shards);
    for (index, item) in manifest.shards.iter_mut().enumerate() {
        item.file = Some(format!("shard-{index:03}.jsonl"));
    }
    let manifest_text = serde_json::to_string_pretty(&manifest.to_value())
        .map_err(|error| format!("manifest JSON：{error}"))?;
    let manifest_path = Path::new(out_dir).join("manifest.json");
    let manifest_path = manifest_path.to_str().ok_or("清单路径不是 UTF-8")?;
    atomic_write(
        manifest_path,
        &format!(
            "{manifest_text}
"
        ),
    )?;
    emit(&json!({
        "shards": shards.shard_count(),
        "entries": shards.len(),
        "head": format!("{:016x}", shards.head()),
        "out": out_dir,
        "manifest": manifest_path,
    }))
}

fn read_shard(path: &Path, want: Option<u64>) -> Result<String, String> {
    let text_path = path.to_str().ok_or("分片路径不是 UTF-8")?;
    let text = read_source(text_path)?;
    if let Some(want) = want {
        let got = text_digest(&text);
        if got != want {
            return Err(format!(
                "`{text_path}` 摘要={got:016x}，manifest 记录 {want:016x}"
            ));
        }
    }
    Ok(text)
}

pub(crate) fn cmd_audit_shards(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let dir = one(
        &parsed,
        "audit-shards <dir> [--artifact <initial|->] [--out <path>] [--manifest <path>] [--resume <checkpoint>] [--checkpoint-every N] [--checkpoint-dir <dir>]",
    )?;
    let artifact = parsed.values.get("artifact").map(String::as_str);
    let out = parsed.values.get("out").map(String::as_str);
    let resume_path = parsed.values.get("resume").map(String::as_str);
    let checkpoint_every = opt_usize(&parsed, "checkpoint-every", 0)?;
    let checkpoint_dir = parsed.values.get("checkpoint-dir").map(String::as_str);
    if out.is_some() && artifact.is_none() && resume_path.is_none() {
        return Err("--out 需要 --artifact 或 --resume".into());
    }
    if resume_path.is_some() && artifact.is_some() {
        return Err("--resume 与 --artifact 不能同时用".into());
    }
    if checkpoint_every > 0 && checkpoint_dir.is_none() {
        return Err("--checkpoint-every 需要 --checkpoint-dir".into());
    }
    if checkpoint_every == 0 && checkpoint_dir.is_some() {
        return Err("--checkpoint-dir 需要 --checkpoint-every".into());
    }
    if checkpoint_every > 0 && artifact.is_none() && resume_path.is_none() {
        return Err("--checkpoint-every 需要 --artifact 或 --resume".into());
    }
    let manifest_path = match parsed.values.get("manifest") {
        Some(path) => Some(path.clone()),
        None => {
            let candidate = Path::new(dir).join("manifest.json");
            candidate
                .is_file()
                .then(|| candidate.to_string_lossy().into_owned())
        }
    };
    let manifest = manifest_path
        .as_deref()
        .map(|path| {
            let value: Value = serde_json::from_str(&read_source(path)?)
                .map_err(|error| format!("{path}: {error}"))?;
            ShardManifest::from_value(&value).map_err(err)
        })
        .transpose()?;
    let checkpoint = resume_path
        .map(|path| {
            let value: Value = serde_json::from_str(&read_source(path)?)
                .map_err(|error| format!("{path}: {error}"))?;
            wire::checkpoint_from_value(&value).map_err(err)
        })
        .transpose()?;
    let mut files: BTreeSet<String> = fs::read_dir(dir)
        .map_err(|error| format!("{dir}: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            (name.starts_with("shard-") && name.ends_with(".jsonl")).then_some(name)
        })
        .collect();
    let (paths, digests) = if let Some(manifest) = &manifest {
        let mut paths = Vec::with_capacity(manifest.shards.len());
        let mut digests = Vec::with_capacity(manifest.shards.len());
        let mut listed = BTreeSet::new();
        for item in &manifest.shards {
            let name = item
                .file
                .clone()
                .unwrap_or_else(|| format!("shard-{:03}.jsonl", item.shard));
            if !listed.insert(name.clone()) {
                return Err(format!("manifest 重复列出 `{name}`"));
            }
            if !files.remove(&name) {
                return Err(format!("manifest 列出的 `{name}` 不存在"));
            }
            paths.push(Path::new(dir).join(&name));
            digests.push(Some(item.digest));
        }
        if let Some(extra) = files.into_iter().next() {
            return Err(format!("目录里有清单外的分片 `{extra}`"));
        }
        (paths, digests)
    } else {
        let mut paths: Vec<std::path::PathBuf> = files
            .into_iter()
            .map(|name| Path::new(dir).join(name))
            .collect();
        paths.sort();
        if paths.is_empty() {
            return Err(format!("{dir} 中没有 shard-*.jsonl"));
        }
        let digests = vec![None; paths.len()];
        (paths, digests)
    };
    if let Some(checkpoint) = &checkpoint {
        if checkpoint.shard > paths.len() {
            return Err(format!(
                "检查点 shard={} 超过分片数 {}",
                checkpoint.shard,
                paths.len()
            ));
        }
        let mut prefix = ShardCursor::new();
        for (path, want) in paths.iter().zip(&digests).take(checkpoint.shard) {
            prefix.push(&read_shard(path, *want)?).map_err(err)?;
        }
        if prefix.shard_count() != checkpoint.shard
            || prefix.entries() != checkpoint.entries
            || prefix.head() != checkpoint.head
        {
            return Err(format!(
                "检查点与分片前缀不一致（检查点 shard={} entries={} head={:016x}）",
                checkpoint.shard, checkpoint.entries, checkpoint.head
            ));
        }
    }
    let mut replay = match &checkpoint {
        Some(checkpoint) => Some(ShardReplay::resume(checkpoint)),
        None => artifact
            .map(|path| load_net(path).map(|net| ShardReplay::new(&net)))
            .transpose()?,
    };
    if checkpoint_every > 0 {
        let dir = checkpoint_dir.expect("checked above");
        fs::create_dir_all(dir).map_err(|error| format!("{dir}: {error}"))?;
    }
    let start = checkpoint.as_ref().map_or(0, |checkpoint| checkpoint.shard);
    let mut cursor = ShardCursor::new();
    let mut written = 0usize;
    for (path, want) in paths.iter().zip(&digests).skip(start) {
        let text = read_shard(path, *want)?;
        match &mut replay {
            Some(replay) => {
                replay.push(&text).map_err(err)?;
                let count = replay.shard_count();
                if checkpoint_every > 0 && count % checkpoint_every == 0 {
                    let dir = checkpoint_dir.expect("checked above");
                    let value = wire::checkpoint_to_value(&replay.checkpoint());
                    let text = serde_json::to_string_pretty(&value)
                        .map_err(|error| format!("checkpoint JSON：{error}"))?;
                    let path = Path::new(dir).join(format!("checkpoint-{count:03}.json"));
                    let path = path.to_str().ok_or("检查点路径不是 UTF-8")?;
                    atomic_write(path, &format!("{text}\n"))?;
                    written += 1;
                }
            }
            None => {
                cursor.push(&text).map_err(err)?;
            }
        }
    }
    let (shards, entries, head) = match &replay {
        Some(replay) => (replay.shard_count(), replay.entries(), replay.head()),
        None => (cursor.shard_count(), cursor.entries(), cursor.head()),
    };
    if let Some(manifest) = &manifest {
        if entries != manifest.entries {
            return Err(format!(
                "manifest 条数={}，分片合计 {entries}",
                manifest.entries
            ));
        }
        if head != manifest.head {
            return Err(format!(
                "manifest head={:016x}，分片链头 {head:016x}",
                manifest.head
            ));
        }
    }
    let mut output = json!({
        "valid": true,
        "shards": shards,
        "entries": entries,
        "head": format!("{head:016x}"),
        "manifest_checked": manifest.is_some(),
        "resumed": checkpoint.is_some(),
        "checkpoints": written,
    });
    if let Some(replay) = replay {
        let (report, net) = replay.finish();
        output["replay"] = json!({
            "entries": report.entries,
            "applied": report.applied,
            "rolled_back": report.rolled_back,
            "skipped": report.skipped,
            "behavior_checked": report.behavior_checked,
            "final_checksum": format!("{:016x}", report.final_checksum),
        });
        if let Some(path) = out {
            atomic_write(path, &to_wsp(&net))?;
        }
    }
    emit(&output)
}

pub(crate) fn cmd_replay(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let log_path = one(
        &parsed,
        "replay <log|-> --artifact <initial|-> [--out <path>]",
    )?;
    let artifact = value(&parsed, "artifact")?;
    no_double_stdin(&[log_path, artifact])?;
    let log = AuditLog::from_jsonl(&read_source(log_path)?).map_err(err)?;
    log.verify_chain().map_err(err)?;
    let (replay, net) = log.replay_net(&load_net(artifact)?).map_err(err)?;
    write_out(&parsed, &net)?;
    emit(&json!({
        "entries": replay.entries,
        "applied": replay.applied,
        "rolled_back": replay.rolled_back,
        "skipped": replay.skipped,
        "behavior_checked": replay.behavior_checked,
        "final_checksum": format!("{:016x}", replay.final_checksum),
        "summary": wire::net_summary(&net),
        "artifact": to_wsp(&net),
    }))
}
