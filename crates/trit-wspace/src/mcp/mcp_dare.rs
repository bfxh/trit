//! MCP `wsp_dare_drop`：坐标级 / 行级 DARE 丢弃（从 `mcp.rs` 拆出，控制单文件行数）。

use serde_json::{Value, json};

use crate::{ops, to_wsp, wire};

use super::{err, flag, load, load_key, opt_u64, optional_oracle};

pub(super) fn dare_drop(args: &Value) -> Result<Value, String> {
    let base = load(args)?;
    let tuned = load_key(args, "tuned")?;
    let rate = args
        .get("rate")
        .and_then(Value::as_f64)
        .ok_or("需要 rate（[0, 1) 内的数字）")?;
    let rate = rate as f32;
    let seed = opt_u64(args, "seed", 0);
    let by_row = flag(args, "by_row");
    let block = args
        .get("block")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    if by_row && block.is_some() {
        return Err("by_row 与 block 只能给一个".into());
    }
    let outcome = if by_row {
        crate::dare::dare_drop_rows(&base, &tuned, rate, seed).map_err(err)?
    } else if let Some(block) = block {
        crate::dare::dare_drop_blocks(&base, &tuned, rate, seed, block).map_err(err)?
    } else {
        crate::dare::dare_drop(&base, &tuned, rate, seed).map_err(err)?
    };
    let oracle = optional_oracle(args)?;
    let applied = ops::apply_patch(
        &base,
        outcome.patch.clone(),
        oracle.as_ref(),
        flag(args, "require_before"),
        flag(args, "require_after"),
    )
    .map_err(err)?;
    let mut value = json!({
        "rate": rate,
        "seed": seed,
        "by_row": by_row,
        "block": block,
        "kept": outcome.kept,
        "dropped": outcome.dropped,
        "rescaled_rows": outcome.rescaled_rows,
        "patch": wire::patch_to_value(&outcome.patch),
        "cost": wire::cost_to_value(&outcome.patch.cost(&base)),
        "applied": applied.applied,
        "rolled_back": applied.rolled_back,
        "summary": wire::net_summary(&applied.net),
        "artifact": to_wsp(&applied.net),
    });
    if let Some(oracle) = &oracle {
        value["verify"] = json!({
            "exact": oracle.exact(&applied.net).map_err(err)?,
            "loss": oracle.loss(&applied.net).map_err(err)?,
        });
    }
    Ok(value)
}
