//! 行数据的打包：三值 `row_w`（pos/neg/scale）与 u16 `row_ids`。

use crate::weights::Weights;

/// 三值 `row_w`：每行 16 个 trit 打进 pos/neg 两个 u32，外加一个行 scale。
/// 返回 `(pos, neg, scale, 是否启用)`；未启用时返回占位数组。
pub fn pack_rows(w: &Weights) -> (Vec<u32>, Vec<u32>, Vec<f32>, bool) {
    let packed = std::env::var("WSP_QUANT_ROW")
        .map(|v| v == "sign" || v == "sign_row")
        .unwrap_or(false);
    if !packed {
        return (vec![0], vec![0], vec![0.0], false);
    }
    assert!(w.r <= 32, "三值打包只支持 R ≤ 32");
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
