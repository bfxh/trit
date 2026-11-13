//! CLI `repair`：最小补丁搜索与候选作用域（从 `cli.rs` 拆出）。

use serde_json::json;

use crate::edit::Edit;
mod cli_repair_scope;
use crate::ops;
use crate::repair::RepairOptions;
use crate::{to_wsp, wire};
use cli_repair_scope::{parse_scope_rows, parse_scope_trits, row_scope};

use super::{
    emit, err, load_net, load_oracle, no_double_stdin, one, opt_u64, opt_usize, parse, value,
    write_out,
};

pub(super) fn repair(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &["row-ops"])?;
    let path = one(&parsed, "repair <artifact|-> --oracle <json|->")?;
    let oracle_path = value(&parsed, "oracle")?;
    no_double_stdin(&[path, oracle_path])?;
    let net = load_net(path)?;
    let oracle = load_oracle(oracle_path)?;
    let scope = match (parsed.values.get("rows"), parsed.values.get("trits")) {
        (Some(_), Some(_)) => {
            return Err("--rows 与 --trits 只能给一个（都表示候选编辑的作用域）".into());
        }
        (Some(raw), None) => Some(row_scope(&net, &parse_scope_rows(raw)?)),
        (None, Some(raw)) => Some(parse_scope_trits(raw)?),
        (None, None) => None,
    };
    let keep = parsed
        .values
        .get("keep")
        .map(|source| load_oracle(source))
        .transpose()?;
    let options = RepairOptions {
        max_edits: opt_usize(&parsed, "max-edits", 8)?,
        beam_width: opt_usize(&parsed, "beam-width", 64)?,
        exhaustive_states: opt_u64(&parsed, "exhaustive-states", 200_000)?,
        scope,
        row_ops: parsed.switches.contains("row-ops"),
        keep,
        ..RepairOptions::default()
    };
    match ops::repair_net(&net, &oracle, &options).map_err(err)? {
        Some((found, work)) => {
            write_out(&parsed, &work)?;
            emit(&json!({
                "found": true,
                "repair": wire::repair_to_value(&found),
                "summary": wire::net_summary(&work),
                "artifact": to_wsp(&work),
                "verify": {
                    "exact": oracle.exact(&work).map_err(err)?,
                    "loss": oracle.loss(&work).map_err(err)?,
                },
            }))
        }
        None => emit(&json!({
            "found": false,
            "before_loss": oracle.loss(&net).map_err(err)?,
        })),
    }
}
