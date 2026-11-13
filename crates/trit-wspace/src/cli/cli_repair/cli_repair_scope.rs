//! `repair` 的候选作用域解析（从 `cli_repair.rs` 拆出，控制单文件行数）。

use crate::edit::Edit;
use crate::net::Net;
use crate::trit::Trit;

pub(super) fn parse_scope_rows(raw: &str) -> Result<Vec<(usize, usize)>, String> {
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
pub(super) fn parse_scope_trits(raw: &str) -> Result<Vec<Edit>, String> {
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
pub(super) fn row_scope(net: &Net, rows: &[(usize, usize)]) -> Vec<Edit> {
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
            out.extend(
                Trit::ALL
                    .iter()
                    .filter(|to| **to != current)
                    .map(|&to| Edit::SetTrit {
                        layer,
                        row,
                        col,
                        to,
                    }),
            );
        }
        let current = target.bias_trit(row).expect("row in range");
        out.extend(
            Trit::ALL
                .iter()
                .filter(|to| **to != current)
                .map(|&to| Edit::SetBias { layer, row, to }),
        );
    }
    out
}
