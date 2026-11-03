use crate::mesh::Mesh;

use super::charts::{Atlas, tri_area};

/// UV 质量统计：纹素密度 = UV 面积 × 图集面积 / 世界面积（texel²/单位²）。
///
/// 理想展开时所有三角形密度相同（`p95/p05` → 1）；平面投影在斜面上会被拉伸，
/// 这个比值就是拉伸的直接度量。
#[derive(Clone, Copy, Debug)]
pub struct UvStats {
    pub charts: usize,
    pub median: f32,
    pub p05: f32,
    pub p95: f32,
    /// p95/p05：越大表示密度越不均匀（拉伸越狠）。
    pub spread: f32,
}

pub fn uv_stats(m: &Mesh, a: &Atlas) -> UvStats {
    let mut d: Vec<f32> = Vec::with_capacity(m.tris.len());
    for (fi, t) in m.tris.iter().enumerate() {
        let wa = tri_area(m, t);
        if wa < 1e-12 {
            continue;
        }
        let p = a.uv[fi];
        let area_uv = 0.5
            * ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                + (p[2][0] - p[0][0]) * (p[0][1] - p[1][1]))
                .abs();
        let tx = area_uv * (a.w * a.h) as f32 / wa;
        if tx.is_finite() && tx > 0.0 {
            d.push(tx);
        }
    }
    d.sort_by(f32::total_cmp);
    let q = |f: f32| -> f32 {
        if d.is_empty() {
            return 0.0;
        }
        d[((d.len() as f32 - 1.0) * f).round() as usize]
    };
    let (p05, p95) = (q(0.05), q(0.95));
    UvStats {
        charts: a.charts.len(),
        median: q(0.5),
        p05,
        p95,
        spread: if p05 > 0.0 { p95 / p05 } else { f32::INFINITY },
    }
}
