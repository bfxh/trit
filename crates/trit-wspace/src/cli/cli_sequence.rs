//! CLI `sequence`：多任务连续编辑协议（逐步检查 + 失败回滚到上一步）。

use serde_json::Value;

use crate::codec::to_wsp;
use crate::sequence::{self, SequenceStep};
use crate::wire;

use super::{atomic_write, emit, err, load_json, load_net, one, opt_usize, parse, value};

fn steps_of(value: &Value) -> Result<Vec<SequenceStep>, String> {
    let entries = value
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("steps 缺 steps 数组")?;
    let mut steps = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("step")
            .to_string();
        let patch_value = entry
            .get("patch")
            .ok_or_else(|| format!("steps[{index}] 缺 patch"))?;
        let patch = wire::patch_from_value(patch_value).map_err(err)?;
        let keep = match entry.get("keep") {
            Some(value) => value
                .as_array()
                .ok_or_else(|| format!("steps[{index}].keep 需要数组"))?
                .iter()
                .map(wire::oracle_from_value)
                .collect::<Result<Vec<_>, _>>()
                .map_err(err)?,
            None => Vec::new(),
        };
        let edit = entry
            .get("edit")
            .map(wire::oracle_from_value)
            .transpose()
            .map_err(err)?;
        steps.push(SequenceStep {
            name,
            patch,
            keep,
            edit,
        });
    }
    Ok(steps)
}

pub(super) fn sequence(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "sequence <base|-> --steps <json|-> [--allow N] [--out <path>]",
    )?;
    let steps_path = value(&parsed, "steps")?;
    let allow = opt_usize(&parsed, "allow", 0)?;
    let net = load_net(path)?;
    let steps = steps_of(&load_json(steps_path)?)?;
    let outcome = sequence::run_sequence(&net, &steps, allow).map_err(err)?;
    let text = to_wsp(&outcome.final_net);
    if let Some(out) = parsed.values.get("out") {
        atomic_write(out, &text)?;
    }
    let mut value = outcome.to_json();
    value["artifact"] = serde_json::json!(text);
    emit(&value)
}
