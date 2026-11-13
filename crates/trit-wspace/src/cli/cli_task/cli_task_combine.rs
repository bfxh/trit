//! CLI `task-combine`（从 `cli_task.rs` 拆出，控制单文件行数）。

use serde_json::json;

use crate::edit::Patch;
use crate::{to_wsp, wire};

use super::super::{emit, err, load_net, no_double_stdin, one, parse, value, write_out};
use super::parse_lambdas;

pub(crate) fn task_combine(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "task-combine <base|-> --tasks a,b,... --lambdas L,L... [--out <path>]",
    )?;
    let task_paths = parse_paths(value(&parsed, "tasks")?)?;
    let lambdas = parse_lambdas(value(&parsed, "lambdas")?)?;
    let mut sources = vec![path];
    sources.extend(task_paths.iter().map(String::as_str));
    no_double_stdin(&sources)?;
    let base = load_net(path)?;
    let mut tasks = Vec::with_capacity(task_paths.len());
    for task_path in &task_paths {
        let net = load_net(task_path)?;
        tasks.push(crate::task::TaskVector::between(&base, &net).map_err(err)?);
    }
    let merged = crate::scale::combine_tasks(&base, &tasks, &lambdas).map_err(err)?;
    write_out(&parsed, &merged)?;
    let patch = Patch::diff(&base, &merged).map_err(err)?;
    emit(&json!({
        "lambdas": lambdas,
        "summary": wire::net_summary(&merged),
        "patch": wire::patch_to_value(&patch),
        "cost": wire::cost_to_value(&patch.cost(&base)),
        "artifact": to_wsp(&merged),
    }))
}

fn parse_paths(raw: &str) -> Result<Vec<String>, String> {
    let paths: Vec<String> = raw
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect();
    if paths.is_empty() {
        return Err("--tasks 不能为空".into());
    }
    Ok(paths)
}
