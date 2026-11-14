//! CLI `perm-attack`：置换密钥指纹的攻击自测。

use crate::perm_key;

use super::{emit, err, load_net, load_oracle, one, opt_u64, parse, value};

pub(super) fn perm_attack(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "perm-attack <base|-> --oracle <json|-> [--seed N | --permuted <artifact|->]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let permuted = match parsed.values.get("permuted") {
        Some(path) => load_net(path)?,
        None => {
            let seed = opt_u64(&parsed, "seed", 7)?;
            let key = perm_key::from_seed(&net, seed);
            perm_key::apply(&net, &key).map_err(err)?
        }
    };
    let report = perm_key::attack(&net, &permuted, &oracle).map_err(err)?;
    emit(&report.to_json())
}
