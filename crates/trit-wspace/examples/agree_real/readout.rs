//! 只量化读出向量：上一轮把差距拆到 scale 后，剩下的部分是否来自 readout。
//!
//! 权重（丢 scale 三值）与三值输入由 `quad` 给定；这里只把 f32 readout
//! 换成 absmean 三值（丢 scale），其余口径不变。

use trit_wspace::Trit;

use crate::f32ref::{Projected, sign};

/// 一行 f32 的 absmean 三值化（丢 scale）。
fn trit_row(row: &[f32]) -> Vec<f64> {
    let scale = row.iter().map(|v| f64::from(v.abs())).sum::<f64>() / row.len().max(1) as f64;
    row.iter()
        .map(|v| {
            (f64::from(*v) / scale.max(f64::MIN_POSITIVE))
                .round()
                .clamp(-1.0, 1.0)
        })
        .collect()
}

/// 用三值 readout 与新权重、投影输入判定。
pub fn judge(projected: &Projected, weights: &[f64], xk: &[f64], readout: &[f32]) -> Trit {
    let (_, k) = projected.dims();
    let r = trit_row(readout);
    let mut score = 0.0f64;
    for (row, q) in r.iter().enumerate() {
        let w = &weights[row * k..(row + 1) * k];
        score += q * w.iter().zip(xk).map(|(a, b)| a * b).sum::<f64>();
    }
    sign(score)
}
