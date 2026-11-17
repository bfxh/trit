//! 与 GPU `prepare.wgsl` 完全一致的 top-16：值降序，tie 时概念 id 小优先。

use crate::model::J;

pub fn top16(qb: &[f32], c: usize) -> [(usize, f32); J] {
    let mut bv = [-1e30f32; J];
    let mut bi = [0usize; J];
    for (c, &v) in qb.iter().enumerate().skip(1).take(c.saturating_sub(1)) {
        if v > bv[J - 1] {
            let mut pos = J - 1;
            while pos > 0 && bv[pos - 1] < v {
                bv[pos] = bv[pos - 1];
                bi[pos] = bi[pos - 1];
                pos -= 1;
            }
            bv[pos] = v;
            bi[pos] = c;
        }
    }
    let mut out = [(0usize, 0f32); J];
    for i in 0..J {
        out[i] = (bi[i], bv[i]);
    }
    out
}
