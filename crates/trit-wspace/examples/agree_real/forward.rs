//! 真实三值网络的完整前向：k 维输入三值化 + 第一层逐行 sign + readout 三值化。
//!
//! 这一列应精确等于 `trit_vs_f32`（bias 全零、scale 不影响符号判决），
//! 用来确认剩余差距由哪一环产生。

use trit_wspace::Trit;

use crate::f32ref::{Projected, sign};

/// 一行 f64 的 absmean 三值化（丢 scale）。
fn trit_row(row: &[f64]) -> Vec<f64> {
    let scale = row.iter().map(|v| v.abs()).sum::<f64>() / row.len().max(1) as f64;
    row.iter()
        .map(|v| (v / scale.max(f64::MIN_POSITIVE)).round().clamp(-1.0, 1.0))
        .collect()
}

/// 投影 + 取符号后的 k 维输入（f64 化的 ±1/0）。
fn stage_input(input: &[Trit], sub: &[Vec<f64>]) -> Vec<f64> {
    sub.iter()
        .map(|u| {
            let acc: f64 = u
                .iter()
                .zip(input)
                .map(|(a, t)| a * f64::from(t.to_i8()))
                .sum();
            f64::from(sign(acc).to_i8())
        })
        .collect()
}

/// 与 `Layer::forward` 同口径的判定。
pub fn judge(
    projected: &Projected,
    weights: &[f64],
    input: &[Trit],
    sub: &[Vec<f64>],
    readout: &[f32],
) -> Trit {
    let (_, k) = projected.dims();
    let xt = stage_input(input, sub);
    let r = trit_row(&readout.iter().map(|v| f64::from(*v)).collect::<Vec<_>>());
    let mut score = 0.0f64;
    for (row, q) in r.iter().enumerate() {
        let w = &weights[row * k..(row + 1) * k];
        let v = sign(w.iter().zip(&xt).map(|(a, b)| a * b).sum());
        score += q * f64::from(v.to_i8());
    }
    sign(score)
}
