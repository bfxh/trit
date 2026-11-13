//! CLI `apply-candidate`：应用候选补丁（从 `cli.rs` 拆出，守住台账）。

use crate::wire;

use super::{
    err, load_json, load_net, load_oracle, no_double_stdin, one, parse, perform_patch, value,
};

pub(super) fn apply_candidate_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["require-before", "require-after"])?;
    let path = one(
        &parsed,
        "apply-candidate <artifact|-> --candidate <json|-> [--oracle <json|->] [--require-before] [--require-after] [--log <path>]",
    )?;
    let candidate_path = value(&parsed, "candidate")?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, candidate_path];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;
    let net = load_net(path)?;
    let candidate = load_json(candidate_path)?;
    let patch = wire::patch_from_candidate(&candidate).map_err(err)?;
    let oracle = oracle_path.map(load_oracle).transpose()?;
    perform_patch(&parsed, &net, patch, oracle, "cli-apply-candidate")
}
