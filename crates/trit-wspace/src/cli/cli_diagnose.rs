//! CLI `diagnose`：一读诊断。给工件 + 判据，直接回答「行不行、哪里不行、改哪里有好处」。

use serde_json::json;

use crate::diagnose::{DiagnoseOptions, Verdict, diagnose};
use crate::wire;

use super::{emit, err, load_net, load_oracle, no_double_stdin, one, opt_usize, parse, value};

pub(super) fn diagnose_cmd(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
    let path = one(
        &parsed,
        "diagnose <artifact|-> --oracle <json|-> [--budget N] [--top K] [--max-edits N] [--beam-width N]",
    )?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let options = DiagnoseOptions {
        probe_budget: opt_usize(&parsed, "budget", 0)?,
        top: opt_usize(&parsed, "top", 8)?,
        max_edits: opt_usize(&parsed, "max-edits", 8)?,
        beam_width: opt_usize(&parsed, "beam-width", 64)?,
    };
    let diagnosis = diagnose(&net, &oracle, &options).map_err(err)?;
    let weakest = diagnosis.rows.first().map(|row| {
        json!({
            "layer": row.layer,
            "row": row.row,
            "zero_loss": row.zero.after_loss,
            "negate_loss": row.negate.after_loss,
        })
    });
    let best_edit = diagnosis.edits.first().map(|edit| {
        json!({
            "edit": wire::edit_to_value(&edit.edit),
            "gain": edit.gain,
            "fixed": edit.fixed,
            "broken": edit.broken,
        })
    });
    let suggestion = match diagnosis.verdict {
        Verdict::Ok => "已满足判据；看 weakest 里哪一行一碰就坏".to_string(),
        Verdict::Repairable => format!(
            "可修：{} 步补丁（best_edit 是单步最优）",
            diagnosis.repair.patch_len
        ),
        Verdict::NotRepairable => {
            "搜索预算内无解；best_edit 的最高收益也救不回来，建议换任务或换网络".to_string()
        }
    };
    emit(&json!({
        "verdict": diagnosis.verdict.as_str(),
        "arch": diagnosis.arch,
        "trits": diagnosis.trits,
        "samples": diagnosis.samples,
        "exact": diagnosis.exact,
        "loss": diagnosis.loss,
        "rows": diagnosis.rows.iter().map(wire::row_impact_to_value).collect::<Vec<_>>(),
        "edits": diagnosis.edits.iter().map(wire::impact_to_value).collect::<Vec<_>>(),
        "repair": {
            "found": diagnosis.repair.found,
            "patch_len": diagnosis.repair.patch_len,
            "evaluated": diagnosis.repair.evaluated,
        },
        "weakest": weakest,
        "best_edit": best_edit,
        "suggestion": suggestion,
    }))
}
