use serde_json::json;

use crate::net::Net;
use crate::to_wsp;

use super::{atomic_write, cli_agree_stats, emit, err, load_net, one, parse, value};
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
        "before": { "rows": target.rows(), "cols": target.cols() },
        "after": { "rows": projected.rows(), "cols": projected.cols() },
        "agree": cli_agree_stats::agree(&net, &projected_net, basis, &parsed, 3)?,
        "trits": projected_net.trit_count(),
        "out": out,
    }))
}
