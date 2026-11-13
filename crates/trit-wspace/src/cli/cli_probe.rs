//! CLI `probe`：行级消融（从 `cli.rs` 拆出，守住台账）。

use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn probe_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["adaptive"])?;
    let path = one(
        &parsed,
        "probe <artifact|-> --oracle <json|-> [--budget N] [--top K] [--adaptive]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let budget = opt_usize(&parsed, "budget", 0)?;
    let top = opt_usize(&parsed, "top", 16)?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let report = if parsed.switches.contains("adaptive") {
        crate::probe::probe_rows_adaptive(&net, &oracle, budget, top).map_err(err)?
    } else {
        crate::probe::probe_rows(&net, &oracle, budget, top).map_err(err)?
    };
    emit(&wire::probe_report_to_value(&report))
}
