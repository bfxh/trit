//! 均匀位宽 GPTQ 的反量化（只在 `TRIT_UNIFORM_GPTQ=1` 时启用）。

use super::super::F32Linear;
use trit_wspace::Result;

/// 码值与块 scale 来自 `gptq::quantize_grouped_bits`，再反量化回 f32。
pub(crate) fn dequantize_gptq(
    linear: &F32Linear,
    bits: u32,
    group: usize,
    hessian: &[f32],
) -> Result<F32Linear> {
    let group = group.clamp(1, linear.cols);
    let (codes, scales) = trit_wspace::gptq::quantize_grouped_bits(
        &linear.weight,
        linear.rows,
        linear.cols,
        hessian,
        group,
        bits,
    )?;
    let groups = linear.cols.div_ceil(group);
    let mut weight = vec![0.0f32; linear.rows * linear.cols];
    for row in 0..linear.rows {
        for chunk in 0..groups {
            let start = chunk * group;
            let end = (start + group).min(linear.cols);
            let scale = scales[row * groups + chunk];
            for col in start..end {
                weight[row * linear.cols + col] = f32::from(codes[row * linear.cols + col]) * scale;
            }
        }
    }
    Ok(F32Linear {
        rows: linear.rows,
        cols: linear.cols,
        weight,
        bias: linear.bias.clone(),
    })
}
