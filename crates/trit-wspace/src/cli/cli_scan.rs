//! CLI `scan`：两阶段参数扫描（粗筛 + 全量复核）与能力命名。

use serde_json::json;

use crate::attribution::{EditMode, ParamRef};
use crate::capability::name_capabilities;
use crate::scan::{ScanOptions, scan};

use super::{
    Parsed, emit, err, load_json, load_net, load_oracle, no_double_stdin, one, opt_usize, parse,
    value,
};

fn param_json(param: &ParamRef) -> serde_json::Value {
    json!({"layer": param.layer, "row": param.row, "col": param.col})
}

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

pub(super) fn scan_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "scan <artifact|-> --oracle <json|-> [--labels <json|->] [--max-params N] [--sample-budget N] [--refine K] [--top K] [--mode flip|zero] [--layer N] [--row N]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let labels = match parsed.values.get("labels") {
        Some(source) => {
            no_double_stdin(&[source.as_str()])?;
            let labels: Vec<String> = serde_json::from_value(load_json(source)?)
                .map_err(|_| "--labels 需要字符串数组".to_string())?;
            if labels.len() != oracle.len() {
                return Err(format!(
                    "--labels 长度 {} 与样本数 {} 不一致",
                    labels.len(),
                    oracle.len()
                ));
            }
            Some(labels)
        }
        None => None,
    };
    let mode = match parsed.values.get("mode").map(String::as_str) {
        None | Some("flip") => EditMode::Flip,
        Some("zero") => EditMode::Zero,
        Some(other) => return Err(format!("--mode 只能是 flip 或 zero，得到 `{other}`")),
    };
    let options = ScanOptions {
        mode,
        layer: opt_index(&parsed, "layer")?,
        row: opt_index(&parsed, "row")?,
        max_params: opt_usize(&parsed, "max-params", 4096)?,
        top: opt_usize(&parsed, "top", 16)?,
        sample_budget: opt_usize(&parsed, "sample-budget", 0)?,
        refine: opt_usize(&parsed, "refine", 0)?,
    };
    let result = scan(&net, &oracle, &options).map_err(err)?;
    let named = name_capabilities(&oracle, &result.report, labels.as_deref());
    emit(&json!({
        "stats": {
            "total_params": result.stats.total_params,
            "screened": result.stats.screened,
            "refined": result.stats.refined,
            "sampled": result.stats.sampled,
        },
        "samples": result.report.samples,
        "baseline_loss": result.report.baseline_loss,
        "capabilities": named.iter().map(|cap| json!({
            "name": cap.name,
            "label": cap.label,
            "lost": cap.lost,
            "support": cap.support.iter().map(param_json).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "specialists": result.report.specialists.iter().map(param_json).collect::<Vec<_>>(),
    }))
}
