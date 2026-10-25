//! 三值量化与反量化。
//!
//! 三种 scale 选择：`absmean`（BitNet 的默认做法）、`absmax`、以及三电平
//! 高斯 Lloyd-Max 的闭式解 `1.224 * σ`（本文档与代码里的 `optimal`）。

use crate::trit::Trit;

/// scale 的选择方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scheme {
    /// `mean(|w|)`，BitNet b1.58 的默认做法。
    AbsMean,
    /// `max(|w|)`，保证不越界但零占比低。
    AbsMax,
    /// 三电平高斯 Lloyd-Max：`1.224 * std`。
    Optimal,
}

/// 量化质量报告。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct QuantReport {
    /// 相对 Frobenius 误差 `||w - ŵ|| / ||w||`。
    pub rel_err: f64,
    /// 零占比。
    pub zero_ratio: f64,
}

/// 按方案算 scale。
pub fn scale_for(values: &[f32], scheme: Scheme) -> f32 {
    if values.is_empty() {
        return 1.0;
    }
    let n = values.len() as f32;
    match scheme {
        Scheme::AbsMean => {
            let s: f32 = values.iter().map(|v| v.abs()).sum::<f32>() / n;
            if s > 0.0 { s } else { 1e-8 }
        }
        Scheme::AbsMax => {
            let s = values.iter().fold(0.0f32, |a, v| a.max(v.abs()));
            if s > 0.0 { s } else { 1e-8 }
        }
        Scheme::Optimal => {
            let mean = values.iter().sum::<f32>() / n;
            let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
            let sd = var.sqrt();
            if sd > 0.0 { 1.224 * sd } else { 1e-8 }
        }
    }
}

/// 按 scale 量化成三值。
pub fn quantize(values: &[f32], scale: f32) -> Vec<Trit> {
    let inv = if scale > 0.0 { 1.0 / scale } else { 0.0 };
    values
        .iter()
        .map(|v| {
            let q = (v * inv).round();
            Trit::from_i8_unchecked(q.clamp(-1.0, 1.0) as i8)
        })
        .collect()
}

/// 反量化。
pub fn dequantize(trits: &[Trit], scale: f32) -> Vec<f32> {
    trits.iter().map(|t| t.get() as f32 * scale).collect()
}

/// 量化并给出误差报告。
pub fn quantize_with_report(values: &[f32], scheme: Scheme) -> (Vec<Trit>, f32, QuantReport) {
    let scale = scale_for(values, scheme);
    let trits = quantize(values, scale);
    let mut err = 0.0f64;
    let mut norm = 0.0f64;
    let mut zeros = 0usize;
    for (v, t) in values.iter().zip(&trits) {
        let d = (*v - t.get() as f32 * scale) as f64;
        err += d * d;
        norm += (*v as f64) * (*v as f64);
        if t.get() == 0 {
            zeros += 1;
        }
    }
    let report = QuantReport {
        rel_err: if norm > 0.0 { (err / norm).sqrt() } else { 0.0 },
        zero_ratio: if values.is_empty() {
            0.0
        } else {
            zeros as f64 / values.len() as f64
        },
    };
    (trits, scale, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_respects_sign_and_clamps() {
        let v = [0.9f32, -0.9, 0.01, 0.0, 5.0];
        let q = quantize(&v, 0.5);
        assert_eq!(q[0].get(), 1);
        assert_eq!(q[1].get(), -1);
        assert_eq!(q[2].get(), 0);
        assert_eq!(q[3].get(), 0);
        assert_eq!(q[4].get(), 1, "超过 scale 必须截断到 +1");
    }

    #[test]
    fn dequantize_inverts_scale() {
        let t = [Trit::POS, Trit::NEG, Trit::ZERO];
        assert_eq!(dequantize(&t, 0.25), vec![0.25, -0.25, 0.0]);
    }

    #[test]
    fn optimal_scale_matches_lloyd_max() {
        // 12 个独立均匀变量求和减 6 ≈ 标准正态（σ=1），期望 scale ≈ 1.224
        let mut state = 0x1234_5678u64;
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 33) as f32) / ((1u64 << 31) as f32)
        };
        let vals: Vec<f32> = (0..50_000)
            .map(|_| {
                let mut s = 0.0f32;
                for _ in 0..12 {
                    s += next();
                }
                s - 6.0
            })
            .collect();
        let scale = scale_for(&vals, Scheme::Optimal);
        assert!(
            (scale - 1.224).abs() < 0.08,
            "optimal scale = {scale}，应接近 1.224"
        );
    }

    #[test]
    fn report_is_sane() {
        let vals: Vec<f32> = (0..1000).map(|i| ((i % 21) as f32 - 10.0) * 0.01).collect();
        let (trits, _, report) = quantize_with_report(&vals, Scheme::AbsMean);
        assert_eq!(trits.len(), vals.len());
        assert!(report.rel_err > 0.0 && report.rel_err < 1.0);
        assert!(report.zero_ratio > 0.0 && report.zero_ratio < 1.0);
    }
}
