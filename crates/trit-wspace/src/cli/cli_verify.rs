//! `verify` 子命令：工件 + oracle 的精确性检查；默认严格（exact=false 退出非零）。

use serde_json::json;

use crate::oracle::Behavior;
use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn verify(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["strict", "advisory"])?;
    let path = one(
        &parsed,
        "verify <artifact|-> --oracle <json|-> [--limit N] [--strict|--advisory]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let limit = opt_usize(&parsed, "limit", 32)?;
    let exact = oracle.exact(&net).map_err(err)?;
    // 默认严格：exact=false 即退出非零；只有显式 --advisory 才允许“只出报告”。
    if !exact && !parsed.switches.contains("advisory") {
        return Err(format!(
            "verify 不通过：exact=false（loss {}）",
            oracle.loss(&net).map_err(err)?
        ));
    }
    emit(&json!({
        "exact": exact,
        "loss": oracle.loss(&net).map_err(err)?,
        "accuracy": oracle.accuracy(&net).map_err(err)?,
        "signature": format!("{:016x}", Behavior::of(&net, &oracle).map_err(err)?.signature()),
        "summary": wire::net_summary(&net),
        "mismatches": wire::mismatches_value(&net, &oracle, limit).map_err(err)?,
        "classification": wire::classification_value(&net, &oracle).map_err(err)?,
    }))
}

/// `--allow N` 是调用方自报阈值：警告证书不能单独作为合规证据。
pub(super) fn warn_if_allow(allow: usize) {
    if allow > 0 {
        trit_log::error!(
            "警告：--allow {allow} 是调用方自报阈值；证书不能单独作为合规证据，需配合独立策略"
        );
    }
}
