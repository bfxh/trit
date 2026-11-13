//! 三值化损失的拆解：权重一半、输入一半、两半叠加。
//!
//! 与 `f32ref` 的 f32 路径同口径（投影后 `k` 维、readout 留 f32），只把
//! absmean 三值化（**不保留 scale**，与 `Layer` 一致）逐个打开，用来回答
//! 「四成损失里，多少来自量化权重、多少来自量化输入」。

use trit_wspace::Net;
use trit_wspace::Trit;

use crate::f32ref::{Projected, sign};

/// 一行 f64 权重的 absmean 三值化（丢弃 scale，与三值网络一致）。
fn trit_row(row: &[f64]) -> Vec<f64> {
    let scale = row.iter().map(|v| v.abs()).sum::<f64>() / row.len().max(1) as f64;
    if scale == 0.0 {
        return vec![0.0; row.len()];
    }
    row.iter()
        .map(|v| (v / scale).round().clamp(-1.0, 1.0))
        .collect()
}

/// 把投影权重逐行三值化。
pub fn trit_weights(projected: &Projected) -> Vec<f64> {
    let (_, k) = projected.dims();
    projected.weights().chunks(k).flat_map(trit_row).collect()
}

/// 五列一致率：`[f32/f32, w三值, x三值, 两半都三值, 真实三值网络]`。
///
/// `inputs` 是三值化后的原始输入，`tern_out` 是真实三值网络的输出。
pub fn compare(
    base: &[Trit],
    raw: &[&[f32]],
    inputs: &[Vec<Trit>],
    tern_out: &[Vec<Trit>],
    projected: &Projected,
) -> [f64; 5] {
    let w_trit = trit_weights(projected);
    let w = projected.weights();
    let n = base
        .len()
        .min(raw.len())
        .min(inputs.len())
        .min(tern_out.len());
    let mut hits = [0usize; 5];
    for i in 0..n {
        let xk = projected.project_f32(raw[i]);
        let xt = projected.project_trit(&inputs[i]);
        hits[0] += usize::from(projected.judge(w, &xk) == base[i]);
        hits[1] += usize::from(projected.judge(&w_trit, &xk) == base[i]);
        hits[2] += usize::from(projected.judge(w, &xt) == base[i]);
        hits[3] += usize::from(projected.judge(&w_trit, &xt) == base[i]);
        hits[4] += usize::from(tern_out[i].as_slice() == [base[i]]);
    }
    let n = n.max(1) as f64;
    hits.map(|h| h as f64 / n)
}

/// 三值投影网络的输出，以及它与三值学生的一致率。
pub fn agree(
    student: &Net,
    net: &Net,
    inputs: &[Vec<Trit>],
    sub: &[Vec<f64>],
) -> (f64, Vec<Vec<Trit>>) {
    let mut same = 0usize;
    let mut out = Vec::with_capacity(inputs.len());
    for input in inputs {
        let got = net.forward(&reduce(input, sub)).expect("f");
        if student.forward(input).expect("f") == got {
            same += 1;
        }
        out.push(got);
    }
    (same as f64 / inputs.len().max(1) as f64, out)
}

/// 三值输入投影到基的前 `k` 维再取符号（真实三值网络这一层的输入）。
fn reduce(input: &[Trit], sub: &[Vec<f64>]) -> Vec<Trit> {
    sub.iter()
        .map(|u| {
            let mut acc = 0.0f64;
            for (a, value) in u.iter().zip(input) {
                acc += a * f64::from(value.to_i8());
            }
            sign(acc)
        })
        .collect()
}
