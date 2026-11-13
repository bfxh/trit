//! CLI `repair-exhaustive`：对给定验证集穷举 ≤1/≤2 编辑。

use serde_json::json;

use crate::exhaustive::exhaustive;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn exhaustive_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "repair-exhaustive <artifact|-> --oracle <json|-> [--max-edits 1|2] [--max-pairs N]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let max_edits = opt_usize(&parsed, "max-edits", 1)?;
    let max_pairs = opt_usize(&parsed, "max-pairs", 200_000)?;
    let report = exhaustive(&net, &oracle, max_edits, max_pairs).map_err(err)?;
    let conclusion = if report.found {
        format!(
            "存在 ≤{} 步解（穷举 {} 个候选）",
            report.max_edits, report.evaluated
        )
    } else {
        format!(
            "穷举全部 ≤{} 步候选（{} 个），无解；对给定验证集是确定性的",
            report.max_edits, report.evaluated
        )
    };
    emit(&json!({
        "max_edits": report.max_edits,
        "found": report.found,
        "evaluated": report.evaluated,
        "best_after_loss": report.best_after_loss,
        "conclusion": conclusion,
    }))
}
