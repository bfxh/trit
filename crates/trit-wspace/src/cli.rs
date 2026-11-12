//! 命令行入口：`read / verify / patch / repair / diff / merge / mcp`。
//!
//! 输入 `-` 表示 stdin（一次只允许一个输入用 stdin）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::{Value, json};

use crate::audit::AuditLog;
use crate::codec::{from_wsp, to_wsp};
use crate::edit::Patch;
use crate::merge::{merge_aligned, merge_vote};
use crate::net::Net;
use crate::ops;
use crate::oracle::{Behavior, Oracle};
use crate::repair::RepairOptions;
use crate::wire;

mod cli_artifact_shard;
mod cli_block;
mod cli_gguf;
mod cli_learned;
mod cli_safetensors;
mod cli_task;
mod cli_wsp2;
mod fsutil;
mod lock;
mod usage;
pub(crate) use fsutil::atomic_write;
use usage::USAGE;

/// 进程入口：返回退出码（0 成功，1 失败）。
#[must_use]
pub fn entry() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("FAIL {error}");
            1
        }
    }
}

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let _locks = lock::acquire_cli(args)?;
    let Some(command) = args.first().map(String::as_str) else {
        return Err(USAGE.to_string());
    };
    match command {
        "read" => cmd_read(&args[1..]),
        "verify" => cmd_verify(&args[1..]),
        "task-merge" => cli_task::task_merge(&args[1..]),
        "task-scale" => cli_task::task_scale(&args[1..]),
        "task-combine" => cli_task::task_combine(&args[1..]),
        "script" => cmd_script(&args[1..]),
        "patch" => cmd_patch(&args[1..]),
        "apply-candidate" => cmd_apply_candidate(&args[1..]),
        "rebase" => cmd_rebase(&args[1..]),
        "transplant" => cmd_transplant(&args[1..]),
        "dare-drop" => cmd_dare_drop(&args[1..]),
        "repair" => cmd_repair(&args[1..]),
        "propose" => cli_learned::propose(&args[1..]),
        "train-proposer" => cli_learned::train_proposer(&args[1..]),
        "propose-learned" => cli_learned::propose_learned(&args[1..]),
        "explain" => cmd_explain(&args[1..]),
        "probe" => cmd_probe(&args[1..]),
        "localize" => cmd_localize(&args[1..]),
        "audit" => crate::cli_audit::cmd_audit(&args[1..]),
        "shard" => crate::cli_audit::cmd_shard(&args[1..]),
        "audit-shards" => crate::cli_audit::cmd_audit_shards(&args[1..]),
        "replay" => crate::cli_audit::cmd_replay(&args[1..]),
        "diff" => cmd_diff(&args[1..]),
        "merge" => cmd_merge(&args[1..]),
        "list-safetensors" => cli_safetensors::list(&args[1..]),
        "import-safetensors" => cli_safetensors::import(&args[1..]),
        "export-safetensors" => cli_safetensors::export(&args[1..]),
        "import-safetensors-dir" => cli_safetensors::import_dir(&args[1..]),
        "export-safetensors-dir" => cli_safetensors::export_dir(&args[1..]),
        "import-gguf" => cli_gguf::import(&args[1..]),
        "export-gguf" => cli_gguf::export(&args[1..]),
        "index-artifact" => cli_block::index_artifact(&args[1..]),
        "read-block" => cli_block::read_block(&args[1..]),
        "split-artifact" => cli_artifact_shard::split(&args[1..]),
        "join-artifact" => cli_artifact_shard::join(&args[1..]),
        "pack-artifact" => cli_wsp2::pack(&args[1..]),
        "unpack-artifact" => cli_wsp2::unpack(&args[1..]),
        "mcp" => crate::mcp::serve(std::io::stdin().lock(), std::io::stdout().lock())
            .map_err(|error| error.to_string()),
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => Err(format!("未知子命令 `{other}`\n{USAGE}")),
    }
}

fn cmd_read(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "read <artifact|-> [--out <path>]")?;
    let net = load_net(path)?;
    write_out(&parsed, &net)?;
    emit(&json!({
        "summary": wire::net_summary(&net),
        "wsp1": to_wsp(&net),
    }))
}

fn cmd_verify(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "verify <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let limit = opt_usize(&parsed, "limit", 32)?;
    emit(&json!({
        "exact": oracle.exact(&net).map_err(err)?,
        "loss": oracle.loss(&net).map_err(err)?,
        "accuracy": oracle.accuracy(&net).map_err(err)?,
        "signature": format!("{:016x}", Behavior::of(&net, &oracle).map_err(err)?.signature()),
        "summary": wire::net_summary(&net),
        "mismatches": wire::mismatches_value(&net, &oracle, limit).map_err(err)?,
        "classification": wire::classification_value(&net, &oracle).map_err(err)?,
    }))
}

fn cmd_script(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "script <artifact|-> --script <json|-> --oracle <json|-> [--out <path>]",
    )?;
    let script_path = value(&parsed, "script")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, script_path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let script = wire::script_from_value(&load_json(script_path)?).map_err(err)?;
    let mut work = net.clone();
    let report = script.run(&mut work, &oracle).map_err(err)?;
    write_out(&parsed, &work)?;
    emit(&json!({
        "report": wire::script_report_to_value(&report),
        "summary": wire::net_summary(&work),
        "artifact": to_wsp(&work),
    }))
}

fn cmd_patch(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["require-before", "require-after"])?;
    let path = one(
        &parsed,
        "patch <artifact|-> --patch <json|-> [--oracle <json|->] [--log <path>]",
    )?;
    let patch_path = value(&parsed, "patch")?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, patch_path];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;

    let net = load_net(path)?;
    let patch = wire::patch_from_value(&load_json(patch_path)?).map_err(err)?;
    let oracle = oracle_path.map(load_oracle).transpose()?;
    perform_patch(&parsed, &net, patch, oracle, "cli-patch")
}

fn cmd_apply_candidate(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["require-before", "require-after"])?;
    let path = one(
        &parsed,
        "apply-candidate <artifact|-> --candidate <json|-> [--oracle <json|->] [--require-before] [--require-after] [--log <path>]",
    )?;
    let candidate_path = value(&parsed, "candidate")?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, candidate_path];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;

    let net = load_net(path)?;
    let candidate = load_json(candidate_path)?;
    let patch = wire::patch_from_candidate(&candidate).map_err(err)?;
    let oracle = oracle_path.map(load_oracle).transpose()?;
    perform_patch(&parsed, &net, patch, oracle, "cli-apply-candidate")
}

fn cmd_rebase(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["skip-conflicts", "require-before", "require-after"])?;
    let path = one(
        &parsed,
        "rebase <target|-> --base <base|-> --patch <json|-> [--skip-conflicts] [--oracle <json|->] [--require-after] [--out <path>]",
    )?;
    let base_path = value(&parsed, "base")?;
    let patch_path = value(&parsed, "patch")?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, base_path, patch_path];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;

    let target = load_net(path)?;
    let base = load_net(base_path)?;
    let patch = wire::patch_from_value(&load_json(patch_path)?).map_err(err)?;
    let oracle = oracle_path.map(load_oracle).transpose()?;
    let skip_conflicts = parsed.switches.contains("skip-conflicts");
    let report = crate::rebase::rebase(&base, &target, &patch, skip_conflicts).map_err(err)?;
    let mut output = wire::rebase_to_value(&report);
    if report.rejected {
        output["summary"] = wire::net_summary(&target);
        output["artifact"] = json!(to_wsp(&target));
        return emit(&output);
    }
    let outcome = ops::apply_patch(
        &target,
        report.clean.clone(),
        oracle.as_ref(),
        parsed.switches.contains("require-before"),
        parsed.switches.contains("require-after"),
    )
    .map_err(err)?;
    write_out(&parsed, &outcome.net)?;
    output["applied"] = json!(outcome.applied);
    output["rolled_back"] = json!(outcome.rolled_back);
    output["cost"] = wire::cost_to_value(&report.clean.cost(&target));
    output["summary"] = wire::net_summary(&outcome.net);
    output["artifact"] = json!(to_wsp(&outcome.net));
    if let Some(oracle) = &oracle {
        output["verify"] = json!({
            "exact": oracle.exact(&outcome.net).map_err(err)?,
            "loss": oracle.loss(&outcome.net).map_err(err)?,
        });
    }
    emit(&output)
}

fn cmd_transplant(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["require-before", "require-after"])?;
    let path = one(
        &parsed,
        "transplant <dest|-> --from <source|-> --layer N --rows i,j [--oracle <json|->] [--require-after] [--out <path>]",
    )?;
    let from = value(&parsed, "from")?;
    let layer = req_usize(&parsed, "layer")?;
    let rows = parse_rows(value(&parsed, "rows")?)?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, from];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;

    let dest = load_net(path)?;
    let source = load_net(from)?;
    let patch = crate::transplant::transplant_units(&source, &dest, layer, &rows).map_err(err)?;
    let oracle = oracle_path.map(load_oracle).transpose()?;
    let cost = patch.cost(&dest);
    let outcome = ops::apply_patch(
        &dest,
        patch.clone(),
        oracle.as_ref(),
        parsed.switches.contains("require-before"),
        parsed.switches.contains("require-after"),
    )
    .map_err(err)?;
    write_out(&parsed, &outcome.net)?;
    let mut output = json!({
        "layer": layer,
        "rows": rows,
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&cost),
        "applied": outcome.applied,
        "rolled_back": outcome.rolled_back,
        "summary": wire::net_summary(&outcome.net),
        "artifact": to_wsp(&outcome.net),
    });
    if let Some(oracle) = &oracle {
        output["verify"] = json!({
            "exact": oracle.exact(&outcome.net).map_err(err)?,
            "loss": oracle.loss(&outcome.net).map_err(err)?,
        });
    }
    emit(&output)
}

fn cmd_dare_drop(args: &[String]) -> Result<(), String> {
    let parsed = parse(
        args,
        &["require-before", "require-after", "by-row", "by-layer"],
    )?;
    let path = one(
        &parsed,
        "dare-drop <base|-> --tuned <tuned|-> --rate R [--seed N] [--by-row|--by-layer|--block N] [--oracle <json|->] [--require-after] [--out <path>]",
    )?;
    let tuned_path = value(&parsed, "tuned")?;
    let rate = req_f32(&parsed, "rate")?;
    let seed = opt_u64(&parsed, "seed", 0)?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, tuned_path];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;

    let base = load_net(path)?;
    let tuned = load_net(tuned_path)?;
    let by_row = parsed.switches.contains("by-row");
    let by_layer = parsed.switches.contains("by-layer");
    let block = if parsed.values.contains_key("block") {
        Some(req_usize(&parsed, "block")?)
    } else {
        None
    };
    if usize::from(by_row) + usize::from(by_layer) + usize::from(block.is_some()) > 1 {
        return Err("--by-row / --by-layer / --block 只能给一个".into());
    }
    let outcome = if by_layer {
        crate::dare::dare_drop_layers(&base, &tuned, rate, seed).map_err(err)?
    } else if by_row {
        crate::dare::dare_drop_rows(&base, &tuned, rate, seed).map_err(err)?
    } else if let Some(block) = block {
        crate::dare::dare_drop_blocks(&base, &tuned, rate, seed, block).map_err(err)?
    } else {
        crate::dare::dare_drop(&base, &tuned, rate, seed).map_err(err)?
    };
    let oracle = oracle_path.map(load_oracle).transpose()?;
    let cost = outcome.patch.cost(&base);
    let applied_outcome = ops::apply_patch(
        &base,
        outcome.patch.clone(),
        oracle.as_ref(),
        parsed.switches.contains("require-before"),
        parsed.switches.contains("require-after"),
    )
    .map_err(err)?;
    write_out(&parsed, &applied_outcome.net)?;
    let mut output = json!({
        "rate": rate,
        "seed": seed,
        "by_row": by_row,
        "by_layer": by_layer,
        "block": block,
        "kept": outcome.kept,
        "dropped": outcome.dropped,
        "rescaled_rows": outcome.rescaled_rows,
        "patch": wire::patch_to_value(&outcome.patch),
        "cost": wire::cost_to_value(&cost),
        "applied": applied_outcome.applied,
        "rolled_back": applied_outcome.rolled_back,
        "summary": wire::net_summary(&applied_outcome.net),
        "artifact": to_wsp(&applied_outcome.net),
    });
    if let Some(oracle) = &oracle {
        output["verify"] = json!({
            "exact": oracle.exact(&applied_outcome.net).map_err(err)?,
            "loss": oracle.loss(&applied_outcome.net).map_err(err)?,
        });
    }
    emit(&output)
}

fn req_usize(parsed: &Parsed, name: &str) -> Result<usize, String> {
    let raw = parsed
        .values
        .get(name)
        .ok_or_else(|| format!("需要 --{name}"))?;
    raw.parse::<usize>()
        .map_err(|_| format!("--{name} 需要非负整数，得到 `{raw}`"))
}

fn req_f32(parsed: &Parsed, name: &str) -> Result<f32, String> {
    let raw = parsed
        .values
        .get(name)
        .ok_or_else(|| format!("需要 --{name}"))?;
    raw.parse::<f32>()
        .map_err(|_| format!("--{name} 需要数字，得到 `{raw}`"))
}

fn parse_rows(raw: &str) -> Result<Vec<usize>, String> {
    if raw.is_empty() {
        return Err("--rows 不能为空".into());
    }
    raw.split(',')
        .map(|part| {
            part.trim()
                .parse::<usize>()
                .map_err(|_| format!("--rows 需要逗号分隔的非负整数，得到 `{part}`"))
        })
        .collect()
}

fn perform_patch(
    parsed: &Parsed,
    net: &Net,
    patch: Patch,
    oracle: Option<Oracle>,
    log_name: &str,
) -> Result<(), String> {
    let require_before = parsed.switches.contains("require-before");
    let require_after = parsed.switches.contains("require-after");
    if (require_before || require_after) && oracle.is_none() {
        return Err("--require-before/--require-after 需要 --oracle".into());
    }
    let patch_for_log = patch.clone();
    let outcome = ops::apply_patch(net, patch, oracle.as_ref(), require_before, require_after)
        .map_err(err)?;
    let applied = outcome.applied;
    let rolled_back = outcome.rolled_back;
    let work = outcome.net;
    if let Some(log_path) = parsed.values.get("log") {
        let mut log = if Path::new(log_path).exists() {
            AuditLog::from_jsonl(
                &fs::read_to_string(log_path).map_err(|error| format!("{log_path}: {error}"))?,
            )
            .map_err(err)?
        } else {
            AuditLog::new()
        };
        log.record_with_oracle(
            log_name,
            net,
            &work,
            patch_for_log,
            applied,
            rolled_back,
            oracle.as_ref(),
        )
        .map_err(err)?;
        atomic_write(log_path, &log.to_jsonl())?;
    }
    write_out(parsed, &work)?;
    let mut output = json!({
        "report": outcome.report,
        "applied": applied,
        "rolled_back": rolled_back,
        "summary": wire::net_summary(&work),
        "artifact": to_wsp(&work),
    });
    if let Some(oracle) = &oracle {
        output["verify"] = json!({
            "exact": oracle.exact(&work).map_err(err)?,
            "loss": oracle.loss(&work).map_err(err)?,
        });
    }
    emit(&output)
}

fn cmd_repair(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "repair <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let options = RepairOptions {
        max_edits: opt_usize(&parsed, "max-edits", 8)?,
        beam_width: opt_usize(&parsed, "beam-width", 64)?,
        exhaustive_pairs: 20_000,
        exhaustive_states: opt_u64(&parsed, "exhaustive-states", 200_000)?,
    };
    match ops::repair_net(&net, &oracle, &options).map_err(err)? {
        Some((found, work)) => {
            write_out(&parsed, &work)?;
            emit(&json!({
                "found": true,
                "repair": wire::repair_to_value(&found),
                "summary": wire::net_summary(&work),
                "artifact": to_wsp(&work),
                "verify": {
                    "exact": oracle.exact(&work).map_err(err)?,
                    "loss": oracle.loss(&work).map_err(err)?,
                },
            }))
        }
        None => emit(&json!({
            "found": false,
            "before_loss": oracle.loss(&net).map_err(err)?,
        })),
    }
}

fn cmd_explain(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "explain <artifact|-> --oracle <json|-> --sample N [--top K]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let sample_raw = value(&parsed, "sample")?;
    let sample = sample_raw
        .parse::<usize>()
        .map_err(|_| format!("--sample 需要非负整数，得到 `{sample_raw}`"))?;
    let top = opt_usize(&parsed, "top", 16)?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let explanation = crate::explain::explain_sample(&net, &oracle, sample, top).map_err(err)?;
    emit(&wire::sample_explanation_to_value(&explanation))
}

fn cmd_probe(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["adaptive"])?;
    let path = one(
        &parsed,
        "probe <artifact|-> --oracle <json|-> [--budget N] [--top K] [--adaptive]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let budget = opt_usize(&parsed, "budget", 0)?;
    let top = opt_usize(&parsed, "top", 16)?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let report = if parsed.switches.contains("adaptive") {
        crate::probe::probe_rows_adaptive(&net, &oracle, budget, top).map_err(err)?
    } else {
        crate::probe::probe_rows(&net, &oracle, budget, top).map_err(err)?
    };
    emit(&wire::probe_report_to_value(&report))
}

fn cmd_localize(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "localize <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let rows = opt_usize(&parsed, "rows", 3)?;
    let row_budget = opt_usize(&parsed, "row-budget", 0)?;
    let max_trits = opt_usize(&parsed, "max-trits", 64)?;
    let top_trits = opt_usize(&parsed, "top-trits", 8)?;
    let report = crate::localize::localize(&net, &oracle, row_budget, rows, max_trits, top_trits)
        .map_err(err)?;
    emit(&wire::localize_report_to_value(&report))
}

fn cmd_diff(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let (left, right) = two(&parsed, "diff <a|-> <b|->")?;
    no_double_stdin(&[left, right])?;
    let a = load_net(left)?;
    let b = load_net(right)?;
    let patch = Patch::diff(&a, &b).map_err(err)?;
    emit(&json!({
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&patch.cost(&a)),
    }))
}

fn cmd_merge(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["aligned"])?;
    let (left, right) = two(&parsed, "merge <a|-> <b|-> [--aligned]")?;
    no_double_stdin(&[left, right])?;
    let a = load_net(left)?;
    let b = load_net(right)?;
    let merged = if parsed.switches.contains("aligned") {
        merge_aligned(&a, &b).map_err(err)?
    } else {
        merge_vote(&a, &b).map_err(err)?
    };
    write_out(&parsed, &merged)?;
    emit(&json!({
        "summary": wire::net_summary(&merged),
        "artifact": to_wsp(&merged),
    }))
}

#[derive(Default)]
pub(crate) struct Parsed {
    pub(crate) positional: Vec<String>,
    pub(crate) values: BTreeMap<String, String>,
    pub(crate) switches: BTreeSet<String>,
}

pub(crate) fn parse(args: &[String], switches: &[&str]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "-" {
            parsed.positional.push(arg.clone());
            index += 1;
            continue;
        }
        if let Some(name) = arg.strip_prefix("--") {
            if switches.contains(&name) {
                parsed.switches.insert(name.to_string());
                index += 1;
            } else {
                let next = args
                    .get(index + 1)
                    .ok_or_else(|| format!("`{arg}` 需要一个值"))?;
                parsed.values.insert(name.to_string(), next.clone());
                index += 2;
            }
        } else {
            parsed.positional.push(arg.clone());
            index += 1;
        }
    }
    Ok(parsed)
}

pub(crate) fn one<'a>(parsed: &'a Parsed, usage: &str) -> Result<&'a str, String> {
    if parsed.positional.len() != 1 {
        return Err(format!("用法：trit-wspace {usage}"));
    }
    Ok(&parsed.positional[0])
}

fn two<'a>(parsed: &'a Parsed, usage: &str) -> Result<(&'a str, &'a str), String> {
    if parsed.positional.len() != 2 {
        return Err(format!("用法：trit-wspace {usage}"));
    }
    Ok((&parsed.positional[0], &parsed.positional[1]))
}

pub(crate) fn value<'a>(parsed: &'a Parsed, name: &str) -> Result<&'a str, String> {
    parsed
        .values
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("需要 --{name}"))
}

pub(crate) fn opt_usize(parsed: &Parsed, name: &str, default: usize) -> Result<usize, String> {
    parsed.values.get(name).map_or(Ok(default), |raw| {
        raw.parse::<usize>()
            .map_err(|_| format!("--{name} 需要非负整数，得到 `{raw}`"))
    })
}

pub(super) fn opt_u64(parsed: &Parsed, name: &str, default: u64) -> Result<u64, String> {
    parsed.values.get(name).map_or(Ok(default), |raw| {
        raw.parse::<u64>()
            .map_err(|_| format!("--{name} 需要非负整数，得到 `{raw}`"))
    })
}

pub(crate) fn no_double_stdin<S: AsRef<str>>(sources: &[S]) -> Result<(), String> {
    let count = sources
        .iter()
        .filter(|source| source.as_ref() == "-")
        .count();
    if count > 1 {
        return Err("一次只允许一个输入来自 stdin（-）".into());
    }
    Ok(())
}

pub(crate) fn read_source(source: &str) -> Result<String, String> {
    if source == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| format!("stdin: {error}"))?;
        Ok(text)
    } else {
        fs::read_to_string(source).map_err(|error| format!("{source}: {error}"))
    }
}

pub(crate) fn load_net(source: &str) -> Result<Net, String> {
    from_wsp(&read_source(source)?).map_err(err)
}

pub(crate) fn load_json(source: &str) -> Result<Value, String> {
    serde_json::from_str(&read_source(source)?).map_err(|error| format!("{source}: {error}"))
}

pub(crate) fn load_oracle(source: &str) -> Result<Oracle, String> {
    wire::oracle_from_value(&load_json(source)?).map_err(err)
}

pub(crate) fn write_out(parsed: &Parsed, net: &Net) -> Result<(), String> {
    if let Some(path) = parsed.values.get("out") {
        atomic_write(path, &to_wsp(net))?;
    }
    Ok(())
}

pub(crate) fn emit(value: &Value) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(value).map_err(|error| format!("JSON 序列化失败：{error}"))?;
    println!("{text}");
    Ok(())
}

pub(crate) fn err(error: crate::error::Error) -> String {
    error.to_string()
}
