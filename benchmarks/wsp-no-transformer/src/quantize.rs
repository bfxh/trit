//! 三值化实现：把 f32 权重压成 {−scale, 0, +scale}。

use crate::quant::Quant;

fn tau_ratio() -> f32 {
    std::env::var("WSP_QUANT_TAU")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.2)
}

/// 对一个切片做三值化；`Sign` 由调用方喂整个矩阵，其余按行喂。
fn quantize_slice(v: &mut [f32], mode: Quant) {
    let threshold = mode == Quant::Threshold;
    let tau = if threshold {
        v.iter().map(|x| x.abs()).fold(0.0, f32::max) * tau_ratio()
    } else {
        0.0
    };
    let kept: Vec<f32> = v
        .iter()
        .copied()
        .filter(|x| if threshold { x.abs() >= tau } else { *x != 0.0 })
        .collect();
    let scale = kept.iter().map(|x| x.abs()).sum::<f32>() / kept.len().max(1) as f32;
    for x in v.iter_mut() {
        *x = if *x == 0.0 || x.abs() < tau {
            0.0
        } else {
            scale * x.signum()
        };
    }
}

pub fn quantize_matrix(v: &mut [f32], rows: usize, cols: usize, mode: Quant) {
    match mode {
        Quant::None => {}
        Quant::Sign => quantize_slice(v, mode),
        Quant::SignRow | Quant::Threshold => {
            for r in 0..rows {
                quantize_slice(&mut v[r * cols..(r + 1) * cols], mode);
            }
        }
    }
}
