//! 行数据的打包。

use crate::weights::Weights;

pub fn pack_rows(w: &Weights) -> (Vec<u32>, Vec<u32>, Vec<f32>, bool) {
    let packed = std::env::var("WSP_QUANT_ROW")
        .map(|v| v == "sign" || v == "sign_row")
        .unwrap_or(false);
    if !packed {
        return (vec![0], vec![0], vec![0.0], false);
    }
    if w.r > 32 {
        return (vec![0], vec![0], vec![0.0], false);
    }
    let mut pos = vec![0u32; w.h];
    let mut neg = vec![0u32; w.h];
    let mut scale = vec![0f32; w.h];
    for row in 0..w.h {
        let base = row * w.r;
        let mut s = 0f32;
        for k in 0..w.r {
            s = s.max(w.row_w[base + k].abs());
        }
        if s == 0.0 {
            continue;
        }
        for k in 0..w.r {
            let v = w.row_w[base + k];
            if v > 0.0 {
                pos[row] |= 1u32 << k;
            } else if v < 0.0 {
                neg[row] |= 1u32 << k;
            }
        }
        scale[row] = s;
    }
    (pos, neg, scale, true)
}

/// 概念 id < 65536（C=4324），两个 u16 打进一个 u32；-1 用 0xFFFF 表示。
pub fn pack_row_ids(ids: &[i64]) -> Vec<u32> {
    let mut packed = vec![0u32; ids.len().div_ceil(2)];
    for (i, &v) in ids.iter().enumerate() {
        let id = if v < 0 { 0xFFFF } else { v as u32 & 0xFFFF };
        packed[i / 2] |= id << ((i % 2) * 16);
    }
    packed
}
