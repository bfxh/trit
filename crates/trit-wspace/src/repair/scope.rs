//! 候选编辑：点编辑 + 可选行级原语（从 `repair.rs` 拆出，控制单文件行数）。

use crate::edit::Edit;
use crate::net::Net;
use crate::trit::Trit;

use super::RepairOptions;

pub(crate) fn scoped_candidates(net: &Net, options: &RepairOptions) -> Vec<Edit> {
    match &options.scope {
        Some(scope) => {
            let mut out = scope.clone();
            if options.row_ops {
                // 只在 scope 覆盖到的行上加行级原语，避免把全网络 384 行都塞进来。
                let mut rows: std::collections::BTreeSet<(usize, usize)> =
                    std::collections::BTreeSet::new();
                for edit in scope {
                    if let Edit::SetTrit { layer, row, .. } = edit {
                        rows.insert((*layer, *row));
                    }
                }
                for &(layer, row) in &rows {
                    out.push(Edit::NegateRow { layer, row });
                    let row_count = net.layer(layer).map_or(0, |weights| weights.rows());
                    if row + 1 < row_count {
                        out.push(Edit::SwapRows {
                            layer,
                            a: row,
                            b: row + 1,
                        });
                    }
                }
            }
            out
        }
        None => candidates(net, options.row_ops),
    }
}

/// 所有点编辑候选（每个三值位置的另外两个取值）。
pub(crate) fn candidates(net: &Net, row_ops: bool) -> Vec<Edit> {
    let mut out = Vec::new();
    for (layer, weights) in net.layers().iter().enumerate() {
        for row in 0..weights.rows() {
            for col in 0..weights.cols() {
                let current = weights.trit(row, col).expect("row/col in range");
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
            let current = weights.bias_trit(row).expect("row in range");
            for &to in &Trit::ALL {
                if to != current {
                    out.push(Edit::SetBias { layer, row, to });
                }
            }
        }
        if row_ops {
            for row in 0..weights.rows() {
                out.push(Edit::NegateRow { layer, row });
                if row + 1 < weights.rows() {
                    out.push(Edit::SwapRows {
                        layer,
                        a: row,
                        b: row + 1,
                    });
                }
            }
        }
    }
    out
}
