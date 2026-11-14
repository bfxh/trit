//! CLI `forgetting`：序列编辑的遗忘曲线（步骤 × 历史 oracle 保持率）。

use serde_json::Value;

use crate::forgetting::{self, ForgettingInput};
use crate::wire;

use super::{emit, err, load_json, load_net, one, parse, value};

fn steps_of(value: &Value) -> Result<Vec<ForgettingInput>, String> {
    let entries = value
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("steps 缺 steps 数组")?;
    let mut steps = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let patch_value = entry
            .get("patch")
            .ok_or_else(|| format!("steps[{index}] 缺 patch"))?;
        let patch = wire::patch_from_value(patch_value).map_err(err)?;
        let own = entry
            .get("oracle")
            .map(wire::oracle_from_value)
            .transpose()
            .map_err(err)?;
        steps.push(ForgettingInput { patch, own });
    }
    Ok(steps)
}

fn history_of(value: &Value) -> Result<Vec<crate::oracle::Oracle>, String> {
    let entries = value
        .get("oracles")
        .and_then(Value::as_array)
        .ok_or("history 缺 oracles 数组")?;
    entries
        .iter()
        .map(wire::oracle_from_value)
        .collect::<Result<Vec<_>, _>>()
        .map_err(err)
}

pub(super) fn forgetting(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "forgetting <base|-> --steps <json|-> --history <json|->",
    )?;
    let steps_path = value(&parsed, "steps")?;
    let history_path = value(&parsed, "history")?;
    let net = load_net(path)?;
    let steps = steps_of(&load_json(steps_path)?)?;
    let history = history_of(&load_json(history_path)?)?;
    let curve = forgetting::forgetting_curve(&net, &steps, &history).map_err(err)?;
    emit(&curve.to_json())
}
