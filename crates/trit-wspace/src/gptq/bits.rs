//! 均匀位宽 GPTQ：与三值版共用 OBS 补偿，只是码值截到 `±qmax`。

use super::{Error, Result, check_shape, damped_inverse};

/// 均匀位宽的 GPTQ：per-group 对称量化到 `±qmax`，`qmax = 2^(bits-1)-1`。
///
/// `scales` 按块内绝对值最大值定标，布局仍是 `(row * groups + g)`；返回的码值
/// 在 `[-qmax, qmax]`，调用方按块 scale 反量化。
pub fn quantize_grouped_bits(
    weight: &[f32],
    rows: usize,
    cols: usize,
    hessian: &[f32],
    group: usize,
    bits: u32,
) -> Result<(Vec<i8>, Vec<f32>)> {
    check_shape(weight, rows, cols, "weight")?;
    check_shape(hessian, cols, cols, "hessian")?;
    if rows == 0 || cols == 0 {
        return Err(Error::EmptyLayer { rows, cols });
    }
    assert!((2..=8).contains(&bits), "均匀位宽要在 2..=8");
    let group = group.clamp(1, cols);
    let groups = cols.div_ceil(group);
    let qmax = ((1i32 << (bits - 1)) - 1) as f32;
    let inverse = damped_inverse(hessian, cols);
    let mut scales = vec![0.0f32; rows * groups];
    for row in 0..rows {
        for chunk in 0..groups {
            let start = chunk * group;
            let end = (start + group).min(cols);
            let max = weight[row * cols + start..row * cols + end]
                .iter()
                .fold(0.0f32, |max, value| max.max(value.abs()));
            scales[row * groups + chunk] = if max == 0.0 { 1.0 } else { max / qmax };
        }
    }
    let mut working = weight.to_vec();
    let mut codes = vec![0i8; rows * cols];
    let mut order: Vec<usize> = (0..cols).collect();
    order.sort_by(|&a, &b| {
        hessian[b * cols + b]
            .partial_cmp(&hessian[a * cols + a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (position, &column) in order.iter().enumerate() {
        let diagonal = inverse[column * cols + column];
        let compensated = diagonal.abs() > 1e-12;
        let chunk = column / group;
        for row in 0..rows {
            let scale = scales[row * groups + chunk];
            let value = working[row * cols + column];
            let quantized = (value / scale).round().clamp(-qmax, qmax);
            codes[row * cols + column] = quantized as i8;
            if !compensated {
                continue;
            }
            let error = f64::from(value - scale * quantized) / diagonal;
            if error == 0.0 {
                continue;
            }
            for &next in &order[position + 1..] {
                let delta = error * inverse[column * cols + next];
                working[row * cols + next] -= delta as f32;
            }
        }
    }
    Ok((codes, scales))
}
