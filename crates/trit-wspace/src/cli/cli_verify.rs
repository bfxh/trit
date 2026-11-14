//! CLI `verify`：行为判据（从 `cli.rs` 拆出，守住台账）。

use serde_json::json;

use crate::oracle::Behavior;
use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn verify_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(&parsed, "verify <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let limit = opt_usize(&parsed, "limit", 32)?;
    emit(&json!({
        "exact": oracle.exact(&net).map_err(err)?,
        "loss": oracle.loss(&net).map_err(err)?,
        "accuracy": oracle.accuracy(&net).map_err(err)?,
        "signature": format!("{:016x}", Behavior::of(&net, &oracle).map_err(err)?.signature()),
        "summary": wire::net_summary(&net),
        "mismatches": wire::mismatches_value(&net, &oracle, limit).map_err(err)?,
        "classification": wire::classification_value(&net, &oracle).map_err(err)?,
    }))
}
