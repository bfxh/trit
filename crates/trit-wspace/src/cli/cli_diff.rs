//! CLI `diff`：两个工件的补丁与成本（从 `cli_project.rs` 拆出，避免与 `project` 共享导入）。

use serde_json::json;

use crate::edit::Patch;
use crate::wire;

use super::{emit, err, load_net, no_double_stdin, parse, two};

pub(super) fn diff(args: &[String]) -> Result<(), String> {
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
