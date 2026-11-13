//! CLI `perturb`：样本突变与输入敏感度。

use serde_json::json;

use crate::perturb::{PerturbMode, perturb};

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn perturb_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "perturb <artifact|-> --oracle <json|-> [--mode flip|zero] [--top K]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let mode = match parsed.values.get("mode").map(String::as_str) {
        None | Some("flip") => PerturbMode::Flip,
        Some("zero") => PerturbMode::Zero,
        Some(other) => return Err(format!("--mode 只能是 flip 或 zero，得到 `{other}`")),
    };
    let top = opt_usize(&parsed, "top", 8)?;
    let report = perturb(&net, &oracle, mode).map_err(err)?;
    let mut positions = report.per_position.clone();
    positions.sort_by_key(|position| std::cmp::Reverse(position.flips));
    let mut samples = report.per_sample.clone();
    samples.sort_by_key(|sample| std::cmp::Reverse(sample.lost));
    emit(&json!({
        "samples": report.samples,
        "width": report.width,
        "baseline_loss": report.baseline_loss,
        "perturbations": report.perturbations,
        "flipped": report.flipped,
        "lost": report.lost,
        "gained": report.gained,
        "top_positions": positions.iter().take(top).map(|position| json!({
            "position": position.position,
            "flips": position.flips,
            "lost": position.lost,
        })).collect::<Vec<_>>(),
        "top_samples": samples.iter().take(top).map(|sample| json!({
            "sample": sample.sample,
            "flips": sample.flips,
            "lost": sample.lost,
        })).collect::<Vec<_>>(),
    }))
}
