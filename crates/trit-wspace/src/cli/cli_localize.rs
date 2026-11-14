//! CLI `localize`：行内定位（从 `cli.rs` 拆出，守住台账）。

use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn localize_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "localize <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let rows = opt_usize(&parsed, "rows", 3)?;
    let row_budget = opt_usize(&parsed, "row-budget", 0)?;
    let max_trits = opt_usize(&parsed, "max-trits", 64)?;
    let top_trits = opt_usize(&parsed, "top-trits", 8)?;
    let report = crate::localize::localize(&net, &oracle, row_budget, rows, max_trits, top_trits)
        .map_err(err)?;
    emit(&wire::localize_report_to_value(&report))
}
