//! CLI `script`：脚本执行（从 `cli.rs` 拆出，守住台账）。

use serde_json::json;

use crate::to_wsp;
use crate::wire;

use super::{
    emit, err, load_json, load_net, load_oracle, no_double_stdin, one, parse, value, write_out,
};

pub(super) fn script_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "script <artifact|-> --script <json|-> --oracle <json|-> [--out <path>]",
    )?;
    let script_path = value(&parsed, "script")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, script_path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let script = wire::script_from_value(&load_json(script_path)?).map_err(err)?;
    let mut work = net.clone();
    let report = script.run(&mut work, &oracle).map_err(err)?;
    write_out(&parsed, &work)?;
    emit(&json!({
        "report": wire::script_report_to_value(&report),
        "summary": wire::net_summary(&work),
        "artifact": to_wsp(&work),
    }))
}
