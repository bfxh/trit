//! CLI `inventory`：组件清单。

use serde_json::json;

use crate::inventory::inventory;

use super::{emit, load_net, one, parse};

pub(super) fn inventory_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "inventory <artifact|->")?;
    let net = load_net(path)?;
    let inv = inventory(&net);
    emit(&json!({
        "arch": inv.arch,
        "layers": inv.layers.iter().map(|layer| json!({
            "layer": layer.layer,
            "rows": layer.rows,
            "cols": layer.cols,
            "trits": layer.trits,
            "bias_nonzero": layer.bias_nonzero,
            "zero_rows": layer.zero_rows,
        })).collect::<Vec<_>>(),
        "neurons": inv.neurons,
        "trits": inv.trits,
        "bias": inv.bias,
        "zero_rows": inv.zero_rows,
    }))
}
