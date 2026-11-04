//! 命令行入口：`read / verify / patch / repair / diff / merge / mcp`。
//!
//! 所有报告输出 JSON；`--out` 以“临时文件 + 改名”落盘 WSP1 工件，避免半写。
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

const USAGE: &str = "\
trit-wspace <command>
  read <artifact|-> [--out <path>]
  verify <artifact|-> --oracle <json|-> [--limit N]
  patch <artifact|-> --patch <json|-> [--oracle <json|->] [--require-before] [--require-after] [--log <path>] [--out <path>]
  repair <artifact|-> --oracle <json|-> [--max-edits N] [--beam-width N] [--exhaustive-states N] [--out <path>]
  propose <artifact|-> --oracle <json|-> [--limit N] [--depth 1|2|3] [--pairs] [--max-pairs N] [--max-triples N]
  audit <log|-> [--artifact <initial|->]
  replay <log|-> --artifact <initial|-> [--out <path>]
  diff <a|-> <b|->
  merge <a|-> <b|-> [--aligned] [--out <path>]
  mcp";

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

/// 执行命令（便于测试）。
pub fn run(args: &[String]) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        return Err(USAGE.to_string());
    };
    match command {
        "read" => cmd_read(&args[1..]),
        "verify" => cmd_verify(&args[1..]),
        "patch" => cmd_patch(&args[1..]),
        "repair" => cmd_repair(&args[1..]),
        "propose" => cmd_propose(&args[1..]),
        "audit" => cmd_audit(&args[1..]),
        "replay" => cmd_replay(&args[1..]),
        "diff" => cmd_diff(&args[1..]),
        "merge" => cmd_merge(&args[1..]),
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
    let patch_for_log = patch.clone();
    let oracle = oracle_path.map(load_oracle).transpose()?;
    let require_before = parsed.switches.contains("require-before");
    let require_after = parsed.switches.contains("require-after");
    if (require_before || require_after) && oracle.is_none() {
        return Err("--require-before/--require-after 需要 --oracle".into());
    }
    let outcome = ops::apply_patch(&net, patch, oracle.as_ref(), require_before, require_after)
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
            "cli-patch",
            &net,
            &work,
            patch_for_log,
            applied,
            rolled_back,
            oracle.as_ref(),
        )
        .map_err(err)?;
        atomic_write(log_path, &log.to_jsonl())?;
    }
    write_out(&parsed, &work)?;
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

fn cmd_audit(args: &[String]) -> Result<(), String> {
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

fn cmd_replay(args: &[String]) -> Result<(), String> {
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

fn cmd_propose(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["pairs"])?;
    let path = one(&parsed, "propose <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let limit = opt_usize(&parsed, "limit", 16)?;
    let depth = match parsed.values.get("depth") {
        Some(raw) => raw
            .parse::<usize>()
            .map_err(|_| format!("--depth 需要 1/2/3，得到 `{raw}`"))?,
        None if parsed.switches.contains("pairs") => 2,
        None => 1,
    };
    match depth {
        1 => {
            let analysis = crate::analyze::analyze(&net, &oracle, limit).map_err(err)?;
            emit(&wire::analysis_to_value(&analysis))
        }
        2 => {
            let max_pairs = opt_usize(&parsed, "max-pairs", 20_000)?;
            let analysis =
                crate::analyze::analyze_pairs(&net, &oracle, limit, max_pairs).map_err(err)?;
            emit(&wire::pair_analysis_to_value(&analysis))
        }
        3 => {
            let max_triples = opt_usize(&parsed, "max-triples", 20_000)?;
            let analysis =
                crate::analyze::analyze_triples(&net, &oracle, limit, max_triples).map_err(err)?;
            emit(&wire::triple_analysis_to_value(&analysis))
        }
        other => Err(format!("--depth 只支持 1/2/3，得到 {other}")),
    }
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
struct Parsed {
    positional: Vec<String>,
    values: BTreeMap<String, String>,
    switches: BTreeSet<String>,
}

fn parse(args: &[String], switches: &[&str]) -> Result<Parsed, String> {
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

fn one<'a>(parsed: &'a Parsed, usage: &str) -> Result<&'a str, String> {
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

fn value<'a>(parsed: &'a Parsed, name: &str) -> Result<&'a str, String> {
    parsed
        .values
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("需要 --{name}"))
}

fn opt_usize(parsed: &Parsed, name: &str, default: usize) -> Result<usize, String> {
    parsed.values.get(name).map_or(Ok(default), |raw| {
        raw.parse::<usize>()
            .map_err(|_| format!("--{name} 需要非负整数，得到 `{raw}`"))
    })
}

fn opt_u64(parsed: &Parsed, name: &str, default: u64) -> Result<u64, String> {
    parsed.values.get(name).map_or(Ok(default), |raw| {
        raw.parse::<u64>()
            .map_err(|_| format!("--{name} 需要非负整数，得到 `{raw}`"))
    })
}

fn no_double_stdin<S: AsRef<str>>(sources: &[S]) -> Result<(), String> {
    let count = sources
        .iter()
        .filter(|source| source.as_ref() == "-")
        .count();
    if count > 1 {
        return Err("一次只允许一个输入来自 stdin（-）".into());
    }
    Ok(())
}

fn read_source(source: &str) -> Result<String, String> {
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

fn load_net(source: &str) -> Result<Net, String> {
    from_wsp(&read_source(source)?).map_err(err)
}

fn load_json(source: &str) -> Result<Value, String> {
    serde_json::from_str(&read_source(source)?).map_err(|error| format!("{source}: {error}"))
}

fn load_oracle(source: &str) -> Result<Oracle, String> {
    wire::oracle_from_value(&load_json(source)?).map_err(err)
}

fn write_out(parsed: &Parsed, net: &Net) -> Result<(), String> {
    if let Some(path) = parsed.values.get("out") {
        atomic_write(path, &to_wsp(net))?;
    }
    Ok(())
}

fn atomic_write(path: &str, text: &str) -> Result<(), String> {
    let temp = format!("{path}.tmp-{}", std::process::id());
    fs::write(&temp, text).map_err(|error| format!("{temp}: {error}"))?;
    match fs::rename(&temp, path) {
        Ok(()) => Ok(()),
        Err(_) => {
            // Windows 上 rename 覆盖已存在文件会失败；先删目标再改名。
            let _ = fs::remove_file(path);
            fs::rename(&temp, path).map_err(|error| {
                let _ = fs::remove_file(&temp);
                format!("{path}: {error}")
            })
        }
    }
}

fn emit(value: &Value) -> Result<(), String> {
    let text =
        serde_json::to_string_pretty(value).map_err(|error| format!("JSON 序列化失败：{error}"))?;
    println!("{text}");
    Ok(())
}

fn err(error: crate::error::Error) -> String {
    error.to_string()
}
