//! CLI `patch`：应用补丁（从 `cli.rs` 拆出，守住台账）。

use crate::wire;

use super::{
    err, load_json, load_net, load_oracle, no_double_stdin, one, parse, perform_patch, value,
};

pub(super) fn patch_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["require-before", "require-after"])?;
    let path = one(
        &parsed,
        "patch <artifact|-> --patch <json|-> [--oracle <json|->] [--log <path>]",
    )?;
    let patch_path = value(&parsed, "patch")?;
    let oracle_path = parsed.values.get("oracle").map(String::as_str);
    let mut sources = vec![path, patch_path];
    if let Some(oracle) = oracle_path {
        sources.push(oracle);
    }
    no_double_stdin(&sources)?;
    let net = load_net(path)?;
    let patch = wire::patch_from_value(&load_json(patch_path)?).map_err(err)?;
    let oracle = oracle_path.map(load_oracle).transpose()?;
    perform_patch(&parsed, &net, patch, oracle, "cli-patch")
}
