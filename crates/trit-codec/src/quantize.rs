//! f32 ↔ trit 三值量化 / 反量化（Ternary Weight Networks 风格）。
//!
//! 把浮点权重 `w ∈ ℝ` 压成 `{-1, 0, +1}`（即 `False / Unknown / True`），
//! 再用一个标量 `scale` 还原回浮点近似值。这是三值网络训练/部署的标准入口：
//! 前向用三值权重乘加，反向把梯度累积回浮点满精度权重（STE 直通）。
//!
//! 阈值规则（Li et al., "Ternary Weight Networks", 2016）：
//!
//! - `scale = max(|wᵢ|)`；空序列或全零时 `scale = 0`。
//! - `threshold = scale * beta`，`beta` 默认 `0.7`（论文默认）。
//! - `wᵢ > threshold` → `True(+scale)`；`wᵢ < -threshold` → `False(-scale)`；
//!   `|wᵢ| ≤ threshold` → `Unknown(0)`。
//!
//! 量化是**有损**的：反量化只取 `±scale` 或 `0`，不保留原浮点值。
//! 这是设计目的，不是缺陷。

use trit_core::value::Ternary;

/// 默认 beta 阈值（TWN 论文默认值）。
pub const DEFAULT_BETA: f32 = 0.7;

/// 量化结果：trit 序列 + 反量化用的 scale。
#[derive(Debug, Clone, PartialEq)]
pub struct Quantized {
    /// 三值权重（False=-1 / Unknown=0 / True=+1）。
    pub trits: Vec<Ternary>,
    /// 反量化系数：True→+scale、False→-scale、Unknown→0。
    pub scale: f32,
    /// 实际使用的 beta（阈值 = scale * beta）。
    pub beta: f32,
}

/// 用默认 beta（0.7）量化。
pub fn quantize(values: &[f32]) -> Quantized {
    quantize_with_beta(values, DEFAULT_BETA)
}

/// 指定 beta 量化。
///
/// `beta ≤ 0` 时全部压成 `Unknown`（scale 仍按 max(|w|) 返回，便于上层观察）；
/// `beta ≥ 1` 时 `threshold ≥ scale`，严格比较下所有权重都落进 Unknown 区间。
///
/// NaN/±inf：`f32::max` 对 NaN 静默跳过，NaN 权重会被压成 Unknown；
/// `scale = inf` 时所有权重重压 ±inf。上层应在调用前清洗数据。
pub fn quantize_with_beta(values: &[f32], beta: f32) -> Quantized {
    let scale = values.iter().map(|w| w.abs()).fold(0.0f32, f32::max);
    let threshold = scale * beta;
    let trits = if beta <= 0.0 {
        // 阈值 0 且严格比较会把非零值都判出来；beta≤0 的语义是"全部压零"。
        vec![Ternary::Unknown; values.len()]
    } else {
        values
            .iter()
            .map(|w| {
                if *w > threshold {
                    Ternary::True
                } else if *w < -threshold {
                    Ternary::False
                } else {
                    Ternary::Unknown
                }
            })
            .collect()
    };
    Quantized { trits, scale, beta }
}

/// 反量化：trit × scale 回 f32。
///
/// `True → +scale`、`False → -scale`、`Unknown → 0`。
pub fn dequantize(trits: &[Ternary], scale: f32) -> Vec<f32> {
    trits
        .iter()
        .map(|t| match t {
            Ternary::True => scale,
            Ternary::False => -scale,
            Ternary::Unknown => 0.0,
        })
        .collect()
}

/// 量化再反量化后的相对误差（L2 归一化），用来评估量化损失。
///
/// `||dequant - original||₂ / ||original||₂`；`original` 全零时返回 0。
pub fn quantization_error(original: &[f32], q: &Quantized) -> f32 {
    let recon = dequantize(&q.trits, q.scale);
    let mut num = 0.0f64;
    let mut den = 0.0f64;
    for (r, o) in recon.iter().zip(original.iter()) {
        let d = (*r - *o) as f64;
        num += d * d;
        den += (*o as f64) * (*o as f64);
    }
    if den == 0.0 {
        0.0
    } else {
        (num / den).sqrt() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_inputs_are_all_unknown_zero_scale() {
        let q = quantize(&[]);
        assert!(q.trits.is_empty());
        assert_eq!(q.scale, 0.0);
    }

    #[test]
    fn all_zero_weights_are_all_unknown() {
        let q = quantize(&[0.0, 0.0, 0.0]);
        assert_eq!(q.trits, vec![Ternary::Unknown; 3]);
        assert_eq!(q.scale, 0.0);
    }

    #[test]
    fn symmetric_distribution_maps_to_three_buckets() {
        // max(|w|) = 1.0，threshold = 0.7。
        // 1.0 > 0.7 → True；-1.0 < -0.7 → False；0.5 ∈ [-0.7,0.7] → Unknown。
        let q = quantize(&[1.0, -1.0, 0.5, -0.5, 0.0]);
        assert_eq!(q.scale, 1.0);
        assert_eq!(
            q.trits,
            vec![
                Ternary::True,
                Ternary::False,
                Ternary::Unknown,
                Ternary::Unknown,
                Ternary::Unknown,
            ]
        );
    }

    #[test]
    fn boundary_value_equal_to_threshold_is_unknown() {
        // max(|w|)=1.0，threshold=0.7。1.0>0.7→True；恰好 0.7 不严格大于→Unknown。
        let q = quantize_with_beta(&[1.0, 0.7, -0.7], 0.7);
        assert_eq!(
            q.trits,
            vec![Ternary::True, Ternary::Unknown, Ternary::Unknown]
        );
    }

    #[test]
    fn dequantize_round_trips_sign() {
        let q = quantize(&[1.0, -1.0, 0.2]);
        let back = dequantize(&q.trits, q.scale);
        assert_eq!(back, vec![1.0, -1.0, 0.0]);
    }

    #[test]
    fn negative_scale_works_for_symmetric_reconstruction() {
        // scale 是 max(|w|)，恒非负；这里只验证 dequantize 接受任意 scale。
        let trits = vec![Ternary::True, Ternary::Unknown, Ternary::False];
        assert_eq!(dequantize(&trits, 2.0), vec![2.0, 0.0, -2.0]);
    }

    #[test]
    fn beta_zero_maps_everything_unknown() {
        let q = quantize_with_beta(&[1.0, -1.0, 0.5], 0.0);
        assert_eq!(q.trits, vec![Ternary::Unknown; 3]);
        // scale 仍然反映 max(|w|)。
        assert_eq!(q.scale, 1.0);
    }

    #[test]
    fn quantization_error_is_small_for_symmetric_data() {
        let original = vec![1.0, -1.0, 1.0, -1.0];
        let q = quantize(&original);
        let err = quantization_error(&original, &q);
        // 这些都超过 0.7 阈值，重建就是 ±1.0，误差应接近 0。
        assert!(err < 0.01, "err = {err}");
    }
}
