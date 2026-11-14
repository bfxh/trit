//! CLI `causal-trace`：激活级因果追踪（corrupt 全域 + 逐层/逐单元 restore）。

use crate::causal_trace;

use super::{emit, err, load_net, load_oracle, one, opt_usize, parse, value};

pub(super) fn causal_trace(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "causal-trace <artifact|-> --oracle <json|-> --sample N",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let sample = opt_usize(&parsed, "sample", 0)?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let trace = causal_trace::causal_trace(&net, &oracle, sample).map_err(err)?;
    emit(&trace.to_json())
}
