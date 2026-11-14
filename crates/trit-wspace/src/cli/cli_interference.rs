//! CLI `interference`：批量编辑的成对干扰矩阵 + 贪心重排。

use crate::interference;
use crate::wire;

use super::{emit, err, load_json, load_net, load_oracle, one, parse, value};

pub(super) fn interference(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "interference <artifact|-> --oracle <json|-> --edits <json|->",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let edits_path = value(&parsed, "edits")?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let patch = wire::patch_from_value(&load_json(edits_path)?).map_err(err)?;
    let report = interference::analyze_interference(&net, &oracle, patch.edits()).map_err(err)?;
    emit(&report.to_json())
}
