//! 均匀位宽权重的量化-反量化（实验用：前向存 f32 反量化值，字节按位宽记账）。
use super::F32Linear;
mod gptq;
pub(crate) use gptq::dequantize_gptq;
/// 对称 per-group RTN：`q = round(w / scale)` 截到 `±qmax`，再反量化回 f32（3-bit 为 ±3，4-bit 为 ±7）。
pub(crate) fn dequantize(linear: &F32Linear, bits: u32, group: usize) -> F32Linear {
    assert!((2..=8).contains(&bits), "均匀位宽要在 2..=8");
    let group = group.clamp(1, linear.cols);
    let qmax = ((1i32 << (bits - 1)) - 1) as f32;
    let mut weight = linear.weight.clone();
    for row in 0..linear.rows {
        let base = row * linear.cols;
        for start in (0..linear.cols).step_by(group) {
            let end = (start + group).min(linear.cols);
            let max = weight[base + start..base + end]
                .iter()
                .fold(0.0f32, |max, value| max.max(value.abs()));
            if max == 0.0 {
                weight[base + start..base + end].fill(0.0);
                continue;
            }
            let scale = max / qmax;
            for value in &mut weight[base + start..base + end] {
                *value = (*value / scale).round().clamp(-qmax, qmax) * scale;
            }
        }
    }
    F32Linear {
        rows: linear.rows,
        cols: linear.cols,
        weight,
        bias: linear.bias.clone(),
    }
}
