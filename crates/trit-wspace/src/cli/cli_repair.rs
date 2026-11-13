//! CLI `repair`：最小补丁搜索与候选作用域（从 `cli.rs` 拆出）。

use serde_json::json;

use crate::edit::Edit;
use crate::net::Net;
use crate::ops;
use crate::repair::RepairOptions;
use crate::trit::Trit;
use crate::{to_wsp, wire};

use super::{
    emit, err, load_net, load_oracle, no_double_stdin, one, opt_u64, opt_usize, parse, value,
    write_out,
};

/// `--rows L:R,L:R` → (layer, row) 列表。
fn parse_scope_rows(raw: &str) -> Result<Vec<(usize, usize)>, String> {
    raw.split(',')
        .map(|part| {
            let (layer, row) = part
                .split_once(':')
                .ok_or_else(|| format!("--rows 需要 L:R，得到 `{part}`"))?;
            Ok((
                layer
                    .parse::<usize>()
                    .map_err(|_| format!("--rows 的层号不是整数：`{part}`"))?,
                row.parse::<usize>()
                    .map_err(|_| format!("--rows 的行号不是整数：`{part}`"))?,
            ))
        })
        .collect()
}

/// `--trits L:R:C:T,...`（T 为 `-`/`0`/`+`）→ 显式候选。
fn parse_scope_trits(raw: &str) -> Result<Vec<Edit>, String> {
    raw.split(',')
        .map(|part| {
            let fields: Vec<&str> = part.split(':').collect();
            if fields.len() != 4 {
                return Err(format!("--trits 需要 L:R:C:T，得到 `{part}`"));
            }
            let layer = fields[0]
                .parse::<usize>()
                .map_err(|_| format!("层号非法：`{part}`"))?;
            let row = fields[1]
                .parse::<usize>()
                .map_err(|_| format!("行号非法：`{part}`"))?;
            let col = fields[2]
                .parse::<usize>()
                .map_err(|_| format!("列号非法：`{part}`"))?;
            let to = match fields[3] {
                "-" => Trit::Neg,
                "0" => Trit::Zero,
                "+" => Trit::Pos,
                other => return Err(format!("目标三值非法：`{other}`")),
            };
            Ok(Edit::SetTrit {
                layer,
                row,
                col,
                to,
            })
        })
        .collect()
}

/// 把 (layer, row) 展开成该行所有 trit（另两个取值）与 bias 的候选。
fn row_scope(net: &Net, rows: &[(usize, usize)]) -> Vec<Edit> {
    let mut out = Vec::new();
    for &(layer, row) in rows {
        let Some(target) = net.layer(layer) else {
            continue;
        };
        if row >= target.rows() {
            continue;
        }
        for col in 0..target.cols() {
            let current = target.trit(row, col).expect("row/col in range");
            for &to in &Trit::ALL {
                if to != current {
                    out.push(Edit::SetTrit {
                        layer,
                        row,
                        col,
                        to,
                    });
                }
            }
        }
        let current = target.bias_trit(row).expect("row in range");
        for &to in &Trit::ALL {
            if to != current {
                out.push(Edit::SetBias { layer, row, to });
            }
        }
    }
    out
}

pub(super) fn repair(args: &[String]) -> Result<(), String> {
    let parsed = parse(args, &[])?;
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
    let options = RepairOptions {
        max_edits: opt_usize(&parsed, "max-edits", 8)?,
        beam_width: opt_usize(&parsed, "beam-width", 64)?,
        exhaustive_pairs: 20_000,
        exhaustive_states: opt_u64(&parsed, "exhaustive-states", 200_000)?,
        scope,
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
