//! scale 的选择：三种口径各自解决什么问题（从 [`super::quantize`] 拆出来，公开路径不变）。
//!
//! - [`Scheme::AbsMean`]：`mean(|w|)`，BitNet b1.58 的默认做法；
//! - [`Scheme::AbsMax`]：`max(|w|)`，保证不越界但零占比低；
//! - [`Scheme::Optimal`]：三电平高斯 Lloyd-Max 的闭式解 `1.224 * σ`。
//!
//! 三条都**不许把 scale 算成 0**：全零或空输入必须有回退值，否则下游
//! `1.0 / scale` 直接毁掉整条量化（退化用例见 `quant.rs` 的测试）。

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
