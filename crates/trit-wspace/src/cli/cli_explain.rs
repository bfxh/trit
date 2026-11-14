//! CLI `explain`：单样本解释（从 `cli.rs` 拆出，守住台账）。

use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn explain_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "explain <artifact|-> --oracle <json|-> --sample N [--top K]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let sample_raw = value(&parsed, "sample")?;
    let sample = sample_raw
        .parse::<usize>()
        .map_err(|_| format!("--sample 需要非负整数，得到 `{sample_raw}`"))?;
    let top = opt_usize(&parsed, "top", 16)?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let explanation = crate::explain::explain_sample(&net, &oracle, sample, top).map_err(err)?;
    emit(&wire::sample_explanation_to_value(&explanation))
}
