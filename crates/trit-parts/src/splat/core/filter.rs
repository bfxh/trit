//! `build_filtered`（拆出 `core.rs` 守模块行数；第 26/31 轮修 depth 越界）。

use std::collections::HashMap;

use crate::fuse::View;

use super::{Acc, Splat};

pub fn build_filtered(
    views: &[View],
    rgbs: &[Vec<f32>],
    cell: f32,
    scale_k: f32,
    keep: impl Fn(u32) -> bool,
) -> Vec<Splat> {
    let mut acc: HashMap<(i32, i32, i32), Acc> = HashMap::new();
    let inv = 1.0 / cell.max(1e-5);
    for (v, rgb) in views.iter().zip(rgbs) {
        if rgb.len() != v.w * v.h * 3 {
            continue;
        }
        for i in 0..v.ids.len().min(v.w * v.h) {
            let Some(&d) = v.depth.get(i) else {
                continue;
            }; // 深度图可能比 id 图短（26/31 轮）
            if v.ids[i] == 0 || !keep(v.ids[i]) || !(d.is_finite() && d > 0.0 && d < 50.0) {
                continue;
            }
            let (u, vv) = (i % v.w, i / v.w);
            let p = v.cam.unproject(u as f64, vv as f64, d);
            let key = (
                (p[0] * inv).floor() as i32,
                (p[1] * inv).floor() as i32,
                (p[2] * inv).floor() as i32,
            );
            let e = acc.entry(key).or_insert(([0.0; 3], [0.0; 3], 0));
            for a in 0..3 {
                e.0[a] += p[a] as f64;
                e.1[a] += rgb[i * 3 + a] as f64;
            }
            e.2 += 1;
        }
    }
    acc.values()
        .map(|(p, c, n)| {
            let n = (*n).max(1) as f64;
            Splat {
                pos: [0, 1, 2].map(|a| (p[a] / n) as f32),
                scale: cell * scale_k,
                color: [0, 1, 2].map(|a| (c[a] / n) as f32),
                alpha: 0.6,
            }
        })
        .collect()
}
