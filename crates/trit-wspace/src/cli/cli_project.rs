//! CLI `project`：用三值基工件投影网络的某一层（输入维度压缩）。

use serde_json::json;

use crate::edit::Patch;
use crate::net::Net;
use crate::to_wsp;
use crate::wire;

use super::{atomic_write, emit, err, load_net, no_double_stdin, one, parse, two, value};
pub(super) fn project(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "project <artifact|-> --basis <artifact> --out <path> [--layer N]",
    )?;
    let basis_path = value(&parsed, "basis")?;
    let out = value(&parsed, "out")?;
    let layer_index = match value(&parsed, "layer") {
        Ok(text) => text
            .parse::<usize>()
            .map_err(|error| format!("--layer: {error}"))?,
        Err(_) => 0,
    };

    let net = load_net(path)?;
    let basis_net = load_net(basis_path)?;
    if basis_net.depth() != 1 {
        return Err(format!(
            "basis 必须是单层，实际 depth={}",
            basis_net.depth()
        ));
    }
    let basis = basis_net.layer(0).ok_or("basis 缺第 0 层")?;
    let target = net
        .layer(layer_index)
        .ok_or_else(|| format!("--layer {layer_index} 越界（depth={}）", net.depth()))?;

    let projected = target.project(basis).map_err(err)?;
    let before = (target.rows(), target.cols());
    let after = (projected.rows(), projected.cols());

    let layers: Vec<_> = net
        .layers()
        .iter()
        .enumerate()
        .map(|(index, layer)| {
            if index == layer_index {
                projected.clone()
            } else {
                layer.clone()
            }
        })
        .collect();
    let projected_net = Net::new(layers).map_err(err)?;

    atomic_write(out, &to_wsp(&projected_net))?;
    emit(&json!({
        "layer": layer_index,
        "before": { "rows": before.0, "cols": before.1 },
        "after": { "rows": after.0, "cols": after.1 },
        "trits": projected_net.trit_count(),
        "out": out,
    }))
}

pub(super) fn cmd_diff(args: &[String]) -> Result<(), String> {
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
