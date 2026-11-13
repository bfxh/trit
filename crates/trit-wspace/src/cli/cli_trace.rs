//! CLI `trace` / `distribution`：前向时序与样本分布。

use serde_json::json;

use crate::distribution::distribution;
use crate::trace::trace;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, parse, value};

pub(super) fn trace_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "trace <artifact|-> --oracle <json|-> --sample N")?;
    let oracle_path = value(&parsed, "oracle")?;
    let sample = value(&parsed, "sample")?
        .parse::<usize>()
        .map_err(|_| "--sample 需要非负整数".to_string())?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let input = oracle
        .inputs()
        .get(sample)
        .ok_or_else(|| format!("--sample {sample} 越界（共 {} 个样本）", oracle.len()))?;
    let report = trace(&net, input).map_err(err)?;
    emit(&json!({
        "sample": sample,
        "input": report.input.iter().map(|trit| trit.to_i8()).collect::<Vec<_>>(),
        "output": report.output.iter().map(|trit| trit.to_i8()).collect::<Vec<_>>(),
        "layers": report.layers.iter().map(|layer| json!({
            "layer": layer.layer,
            "nonzero": layer.nonzero,
            "rows": layer.rows.iter().map(|row| json!({
                "row": row.row,
                "acc": row.acc,
                "out": row.out.to_i8(),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

pub(super) fn distribution_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[oracle_path])?;
    let oracle = load_oracle(oracle_path)?;
    let dist = distribution(&oracle);
    emit(&json!({
        "samples": dist.samples,
        "unique_inputs": dist.unique_inputs,
        "unique_outputs": dist.unique_outputs,
        "inputs": dist.inputs.iter().map(|coord| json!({
            "index": coord.index,
            "neg": coord.neg,
            "zero": coord.zero,
            "pos": coord.pos,
        })).collect::<Vec<_>>(),
        "outputs": dist.outputs.iter().map(|class| json!({
            "class": class.class,
            "count": class.count,
        })).collect::<Vec<_>>(),
    }))
}
