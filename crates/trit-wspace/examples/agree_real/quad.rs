//! 三值化损失的拆解：权重一半、输入一半、两半叠加，以及「丢 scale」的贡献。
//!
//! 与 `f32ref` 的 f32 路径同口径（投影后 `k` 维、readout 留 f32），逐个打开
//! absmean 三值化：先看权重/输入各自，再把「保留 scale」与「丢弃 scale」
//! （与 `Layer` 一致）分开，回答高 k 剩余差距里 scale 占多少。

use trit_wspace::Net;
use trit_wspace::Trit;

use crate::f32ref::{Projected, sign};
use crate::forward;
use crate::readout::judge as judge_readout;

/// 把投影权重逐行 absmean 三值化：`(丢 scale, 保留 scale)`。
pub fn trit_weights(projected: &Projected) -> (Vec<f64>, Vec<f64>) {
    let (_, k) = projected.dims();
    let (mut signed, mut scaled) = (Vec::new(), Vec::new());
    for row in projected.weights().chunks(k) {
        let scale = row.iter().map(|v| v.abs()).sum::<f64>() / row.len().max(1) as f64;
        for v in row {
            let q = (v / scale.max(f64::MIN_POSITIVE)).round().clamp(-1.0, 1.0);
            signed.push(q);
            scaled.push(q * scale);
        }
    }
    (signed, scaled)
}

/// 八列一致率：
/// `[f32/f32, w三值, x三值, 两半都三值, 两半保留scale, readout三值, k维输入三值, 真实网络]`。
pub fn compare(
    base: &[Trit],
    raw: &[&[f32]],
    inputs: &[Vec<Trit>],
    tern_out: &[Vec<Trit>],
    readout: &[f32],
    sub: &[Vec<f64>],
    projected: &Projected,
) -> [f64; 8] {
    let (w_trit, w_scaled) = trit_weights(projected);
    let w = projected.weights();
    let n = base.len().min(raw.len()).min(inputs.len());
    let mut hits = [0usize; 8];
    for i in 0..n {
        let xk = projected.project_f32(raw[i]);
        let xt = projected.project_trit(&inputs[i]);
        hits[0] += usize::from(projected.judge(w, &xk) == base[i]);
        hits[1] += usize::from(projected.judge(&w_trit, &xk) == base[i]);
        hits[2] += usize::from(projected.judge(w, &xt) == base[i]);
        hits[3] += usize::from(projected.judge(&w_trit, &xt) == base[i]);
        hits[4] += usize::from(projected.judge(&w_scaled, &xt) == base[i]);
        hits[5] += usize::from(judge_readout(projected, &w_trit, &xt, readout) == base[i]);
        hits[6] +=
            usize::from(forward::judge(projected, &w_trit, &inputs[i], sub, readout) == base[i]);
        hits[7] += usize::from(tern_out[i].as_slice() == [base[i]]);
    }
    hits.map(|h| h as f64 / n.max(1) as f64)
}

/// 三值投影网络的输出，以及它与三值学生的一致率。
pub fn agree(
    student: &Net,
    net: &Net,
    inputs: &[Vec<Trit>],
    sub: &[Vec<f64>],
) -> (f64, Vec<Vec<Trit>>) {
    let (mut same, mut out) = (0usize, Vec::with_capacity(inputs.len()));
    for input in inputs {
        let got = net.forward(&reduce(input, sub)).expect("f");
        if student.forward(input).expect("f") == got {
            same += 1;
        }
        out.push(got);
    }
    (same as f64 / inputs.len().max(1) as f64, out)
}

/// 三值输入投影到基的前 `k` 维再取符号（真实三值网络第一层的输入）。
fn reduce(input: &[Trit], sub: &[Vec<f64>]) -> Vec<Trit> {
    sub.iter()
        .map(|u| {
            sign(
                u.iter()
                    .zip(input)
                    .map(|(a, t)| a * f64::from(t.to_i8()))
                    .sum(),
            )
        })
        .collect()
}
