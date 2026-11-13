//! dimsweep 的稀疏化曲线：只保留 `|w_eff|` 最大的 top-m 列，其余列置零。
//!
//! 与 `curve.rs` 的投影降维对照：投影让所有列都参与（只是被压扁），稀疏化只让少数列参与。
//! 同一个 m 上两者的差距，就是「是压缩方式错了，还是信息本来就不够」的判据。

use crate::basis::{oracle_of, readout_layer};
use crate::ctx::CurveCtx;
use crate::evaluate::{agreement_with_full, identity, losses};
use serde_json::{Value, json};
use trit_wspace::safetensors::Weights;
use trit_wspace::{Layer, Net, Trit};

/// 稀疏化要扫的列数。
pub const PRUNES: [usize; 7] = [16, 32, 64, 96, 128, 192, 256];

/// 只保留 `keep` 里的列，其余列的三值权重置零；bias 与读出层不变。
pub fn pruned_net(query: &Weights, readout: &[f32], dim: usize, keep: &[usize]) -> Net {
    let base = query.to_layer().expect("ternary query");
    let bias: Vec<i8> = base.bias().iter().map(|t| t.to_i8()).collect();
    let mut mask = vec![false; dim];
    for &j in keep {
        mask[j] = true;
    }
    let mut trits = vec![0i8; query.rows * dim];
    for row in 0..query.rows {
        for (j, slot) in trits[row * dim..(row + 1) * dim].iter_mut().enumerate() {
            if mask[j] {
                *slot = base.trit(row, j).expect("trit").to_i8();
            }
        }
    }
    let first = Layer::new(query.rows, dim, &trits, &bias).expect("pruned layer");
    Net::new(vec![first, readout_layer(readout)]).expect("pruned net")
}

/// 按 `|w_eff|` 降序返回列索引。
pub fn column_order(query: &Weights, readout: &[f32], dim: usize) -> Vec<usize> {
    let mut score = vec![0.0f64; dim];
    for (row, r) in readout.iter().enumerate() {
        let w = &query.values[row * dim..(row + 1) * dim];
        for (j, value) in w.iter().enumerate() {
            score[j] += f64::from(*r) * f64::from(*value);
        }
    }
    let mut order: Vec<usize> = (0..dim).collect();
    order.sort_by(|a, b| {
        score[*b]
            .abs()
            .partial_cmp(&score[*a].abs())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order
}

/// 实际参与前向的非零 trit 数（置零列不算）。
pub fn nonzero_trits(net: &Net) -> usize {
    net.layers()
        .iter()
        .map(|layer| layer.trits().iter().filter(|t| **t != Trit::Zero).count())
        .sum()
}

/// 稀疏化曲线：每个 m 报 edit/keep 与对全维学生的保真度。
pub fn prune_curve(cx: &CurveCtx) -> Vec<Value> {
    let dim = cx.dim;
    let order = column_order(cx.query, cx.readout, dim);
    let basis = identity(dim);
    let edit = oracle_of(cx.edit, &basis);
    let keep = oracle_of(cx.keep, &basis);
    PRUNES
        .iter()
        .filter(|m| **m <= dim)
        .map(|&m| {
            let net = pruned_net(cx.query, cx.readout, dim, &order[..m]);
            let (edit_loss, keep_loss) = losses(&net, &edit, &keep);
            json!({
                "m": m,
                "edit_loss": edit_loss,
                "keep_loss": keep_loss,
                "edit_acc": edit.accuracy(&net).expect("acc"),
                "keep_acc": keep.accuracy(&net).expect("acc"),
                "agree_full": agreement_with_full(&net, cx.samples, &basis, cx.student),
                "keep_cols": m,
                "nonzero_trits": nonzero_trits(&net),
            })
        })
        .collect()
}
