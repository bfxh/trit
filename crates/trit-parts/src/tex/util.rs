//! quilt 内部用的小工具：splitmix64、亮度、平面拟合。
//!
//! 单独成文是为了给 `quilt.rs` 腾预算（322 顶格），公开面零变化。

/// splitmix64：给 patch 搜索用（`seed | 1` 会丢最低位，2026-11-01 踩过）。
pub(crate) fn sm64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub(crate) fn luma_of(p: &[f32]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// 逐通道线性平面拟合 `a + bx·u + by·v`（u,v ∈ `[0,1]`），最小二乘闭式解（`--relight-plane` 用）。
pub(crate) fn fit_plane(src: &[f32], w: usize, h: usize) -> [[f32; 3]; 3] {
    let n = (w * h) as f64;
    let (mut su, mut sv, mut suu, mut svv, mut suv) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut sy = [0.0f64; 3];
    let mut suy = [0.0f64; 3];
    let mut svy = [0.0f64; 3];
    for y in 0..h {
        let v = y as f64 / h.max(1) as f64;
        for x in 0..w {
            let u = x as f64 / w.max(1) as f64;
            su += u;
            sv += v;
            suu += u * u;
            svv += v * v;
            suv += u * v;
            let i = (y * w + x) * 3;
            for c in 0..3 {
                let val = src[i + c] as f64;
                sy[c] += val;
                suy[c] += val * u;
                svy[c] += val * v;
            }
        }
    }
    let inv = inv3([[n, su, sv], [su, suu, suv], [sv, suv, svv]]);
    let mut out = [[0.0f32; 3]; 3];
    for c in 0..3 {
        let b = [sy[c], suy[c], svy[c]];
        for (out_cell, &inv_row) in out[c].iter_mut().zip(inv.iter()) {
            *out_cell = (inv_row[0] * b[0] + inv_row[1] * b[1] + inv_row[2] * b[2]) as f32;
        }
    }
    out
}

fn inv3(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = if det.abs() < 1e-12 { 1e-12 } else { det };
    [
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) / d,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / d,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / d,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) / d,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / d,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / d,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) / d,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / d,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / d,
        ],
    ]
}
