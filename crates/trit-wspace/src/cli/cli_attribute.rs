//! CLI `attribute`：单参数能力归属（改一个 trit 会失去哪些样本/能力）。

use serde_json::json;

use crate::attribution::{AttributeOptions, EditMode, attribute};

use super::{
    Parsed, emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value,
};

fn opt_index(parsed: &Parsed, name: &str) -> Result<Option<usize>, String> {
    parsed
        .values
        .get(name)
        .map(|raw| {
            raw.parse::<usize>()
                .map_err(|_| format!("--{name} 需要非负整数，得到 `{raw}`"))
        })
        .transpose()
}

fn param_json(param: &crate::attribution::ParamRef) -> serde_json::Value {
    json!({"layer": param.layer, "row": param.row, "col": param.col})
}

pub(super) fn attribute_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "attribute <artifact|-> --oracle <json|-> [--mode flip|zero] [--layer N] [--row N] [--max-params N] [--top K]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let mode = match parsed.values.get("mode").map(String::as_str) {
        None | Some("flip") => EditMode::Flip,
        Some("zero") => EditMode::Zero,
        Some(other) => return Err(format!("--mode 只能是 flip 或 zero，得到 `{other}`")),
    };
    let options = AttributeOptions {
        mode,
        layer: opt_index(&parsed, "layer")?,
        row: opt_index(&parsed, "row")?,
        max_params: opt_usize(&parsed, "max-params", 4096)?,
        top: opt_usize(&parsed, "top", 16)?,
    };
    let report = attribute(&net, &oracle, &options).map_err(err)?;
    emit(&json!({
        "samples": report.samples,
        "params": report.params,
        "baseline_loss": report.baseline_loss,
        "top": report.top.iter().map(|a| json!({
            "param": param_json(&a.param),
            "from": a.from.to_i8(),
            "to": a.to.to_i8(),
            "lost": a.lost,
            "gained": a.gained,
            "changed": a.changed,
            "specificity": a.specificity,
            "sole": a.sole,
        })).collect::<Vec<_>>(),
        "capabilities": report.capabilities.iter().map(|cap| json!({
            "lost": cap.lost,
            "support": cap.support.iter().map(param_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "specialists": report.specialists.iter().map(param_json).collect::<Vec<_>>(),
    }))
}
