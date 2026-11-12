//! CLI `propose-gradient`：STE 反向传播的梯度知情排序。

use serde_json::json;

use crate::learned;
use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn propose_gradient(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["verify"])?;
    let path = one(
        &parsed,
        "propose-gradient <artifact|-> --oracle <json|-> [--limit N] [--verify]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    let limit = opt_usize(&parsed, "limit", 16)?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let before_loss = oracle.loss(&net).map_err(err)?;
    let mut scored = learned::gradient_scores(&net, &oracle).map_err(err)?;
    let evaluated = scored.len();
    if limit > 0 {
        scored.truncate(limit);
    }
    let verify = parsed.switches.contains("verify");
    let mut candidates = Vec::with_capacity(scored.len());
    for scored in &scored {
        let mut item = json!({
            "edit": wire::edit_to_value(&scored.edit),
            "score": scored.score,
        });
        if verify {
            let mut trial = net.clone();
            scored.edit.apply_to(&mut trial).map_err(err)?;
            let after_loss = oracle.loss(&trial).map_err(err)?;
            item["after_loss"] = json!(after_loss);
            item["gain"] = json!(before_loss as isize - after_loss as isize);
        }
        candidates.push(item);
    }
    emit(&json!({
        "before_loss": before_loss,
        "evaluated": evaluated,
        "limit": limit,
        "verified": verify,
        "method": "ste-gradient",
        "candidates": candidates,
    }))
}
