//! CLI `impact`：多参数/结构化改动的联合收益。

use serde_json::json;

use crate::impact::{GroupKind, GroupMode, group_edits, impact};
use crate::wire;

use super::{emit, err, load_json, load_net, load_oracle, no_double_stdin, one, parse, value};

pub(super) fn impact_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "impact <artifact|-> --oracle <json|-> (--edits <json|-> | --group row|layer --layer N [--row N] [--mode flip|zero])",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let edits = if let Some(source) = parsed.values.get("edits") {
        no_double_stdin(&[source.as_str()])?;
        wire::patch_from_value(&load_json(source)?)
            .map_err(err)?
            .edits()
            .to_vec()
    } else {
        let kind = match value(&parsed, "group")? {
            "row" => GroupKind::Row,
            "layer" => GroupKind::Layer,
            other => return Err(format!("--group 只能是 row 或 layer，得到 `{other}`")),
        };
        let layer = value(&parsed, "layer")?
            .parse::<usize>()
            .map_err(|_| "--layer 需要非负整数".to_string())?;
        let row = parsed
            .values
            .get("row")
            .map(|raw| raw.parse::<usize>())
            .transpose()
            .map_err(|_| "--row 需要非负整数".to_string())?;
        let mode = match parsed.values.get("mode").map(String::as_str) {
            None | Some("flip") => GroupMode::Flip,
            Some("zero") => GroupMode::Zero,
            Some(other) => return Err(format!("--mode 只能是 flip 或 zero，得到 `{other}`")),
        };
        group_edits(&net, kind, layer, row, mode).map_err(err)?
    };
    let report = impact(&net, &oracle, &edits).map_err(err)?;
    emit(&json!({
        "edits": edits.len(),
        "before_loss": report.before_loss,
        "after_loss": report.after_loss,
        "gain": report.gain,
        "synergy": report.synergy,
        "lost": report.lost,
        "gained": report.gained,
        "changed": report.changed,
        "individual": report.individual.iter().map(|item| json!({
            "edit": wire::edit_to_value(&item.edit),
            "gain": item.gain,
            "lost": item.lost,
            "gained": item.gained,
            "changed": item.changed,
        })).collect::<Vec<_>>(),
    }))
}
