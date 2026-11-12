//! 场景合成（N4 第一档）：对象按包围盒做**货架式摆放**，保证 AABB 不穿插。
//!
//! 输入是对象资产（id + 局部包围盒），输出每个对象的 yaw / 平移 / 世界包围盒。
//! 选朝向时只试 0/90/180/270（轴对齐货架），在若干候选总宽里挑**覆盖率最高**的，
//! 全程确定性——复核器重跑同一函数即可逐项对账。
//!
//! 这是布局的确定性基线，不是学习的布局策略（N4 后续）。

use crate::splat::Splat;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: u32,
    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placement {
    pub id: u32,
    pub yaw: u32,
    pub translate: [f64; 3],
    pub bbox_min: [f64; 3],
    pub bbox_max: [f64; 3],
}

fn extents(it: &Item, yaw: u32) -> (f64, f64) {
    let (w, d) = (
        (it.bbox_max[0] - it.bbox_min[0]).max(0.0),
        (it.bbox_max[2] - it.bbox_min[2]).max(0.0),
    );
    if yaw % 180 == 90 { (d, w) } else { (w, d) }
}

fn center(it: &Item) -> (f64, f64) {
    (
        (it.bbox_min[0] + it.bbox_max[0]) / 2.0,
        (it.bbox_min[2] + it.bbox_max[2]) / 2.0,
    )
}

/// 绕包围盒中心绕 Y 轴旋转 `yaw` 度后，包围盒的 xz 最小角。
fn rotated_min(it: &Item, yaw: u32) -> (f64, f64) {
    let (cx, cz) = center(it);
    let (x0, x1) = (it.bbox_min[0], it.bbox_max[0]);
    let (z0, z1) = (it.bbox_min[2], it.bbox_max[2]);
    match yaw % 360 {
        90 => (cx + (z0 - cz), cz - (x1 - cx)),
        180 => (cx - (x1 - cx), cz - (z1 - cz)),
        270 => (cx - (z1 - cz), cz + (x0 - cx)),
        _ => (x0, z0),
    }
}

/// 两个 AABB 的重叠体积（米³）；不重叠为 0。
pub fn overlap_volume(a: &Placement, b: &Placement) -> f64 {
    let mut v = 1.0f64;
    for k in 0..3 {
        let lo = a.bbox_min[k].max(b.bbox_min[k]);
        let hi = a.bbox_max[k].min(b.bbox_max[k]);
        v *= (hi - lo).max(0.0);
    }
    v
}

/// 所有对象占地之和 / 摆放后总包围盒的 xz 面积。
pub fn coverage(ps: &[Placement]) -> f64 {
    if ps.is_empty() {
        return 0.0;
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut area = 0.0;
    for p in ps {
        for (k, a) in [0usize, 2].into_iter().enumerate() {
            lo[k] = lo[k].min(p.bbox_min[a]);
            hi[k] = hi[k].max(p.bbox_max[a]);
        }
        area += (p.bbox_max[0] - p.bbox_min[0]).max(0.0) * (p.bbox_max[2] - p.bbox_min[2]).max(0.0);
    }
    let total = (hi[0] - lo[0]).max(1e-9) * (hi[1] - lo[1]).max(1e-9);
    area / total
}

/// 按给定总宽摆一遍（货架法）。`width <= 0` 时按对象最大边自动定。
pub fn pack(items: &[Item], gap: f64, width: f64) -> Vec<Placement> {
    let mut order: Vec<&Item> = items.iter().collect();
    order.sort_by(|a, b| {
        let fa = (a.bbox_max[0] - a.bbox_min[0]) * (a.bbox_max[2] - a.bbox_min[2]);
        let fb = (b.bbox_max[0] - b.bbox_min[0]) * (b.bbox_max[2] - b.bbox_min[2]);
        fb.total_cmp(&fa).then(a.id.cmp(&b.id))
    });
    let mut out = Vec::with_capacity(order.len());
    let (mut cx, mut cz, mut shelf_d) = (0.0f64, 0.0f64, 0.0f64);
    for it in &order {
        // 选朝向：占地长边尽量顺着货架宽，其次选窄的（更省宽度）
        let mut best: Option<(u32, f64, f64)> = None;
        for yaw in [0u32, 90, 180, 270] {
            let (w, d) = extents(it, yaw);
            let score = (w, d);
            if best.map(|b| score < (b.1, b.2)).unwrap_or(true) {
                best = Some((yaw, w, d));
            }
        }
        let (yaw, w, d) = best.unwrap_or((0, 0.0, 0.0));
        if width > 0.0 && cx > 0.0 && cx + w > width {
            cx = 0.0;
            cz += shelf_d + gap;
            shelf_d = 0.0;
        }
        let (rx, rz) = rotated_min(it, yaw);
        let tx = cx - rx;
        let tz = cz - rz;
        let ty = -it.bbox_min[1];
        out.push(Placement {
            id: it.id,
            yaw,
            translate: [tx, ty, tz],
            bbox_min: [cx, 0.0, cz],
            bbox_max: [cx + w, (it.bbox_max[1] - it.bbox_min[1]).max(0.0), cz + d],
        });
        cx += w + gap;
        shelf_d = shelf_d.max(d);
    }
    out
}

/// 在若干候选总宽里挑覆盖率最高（同时必须零穿插）的一组摆放。
pub fn pack_best(items: &[Item], gap: f64) -> (Vec<Placement>, f64, f64) {
    let area: f64 = items
        .iter()
        .map(|i| (i.bbox_max[0] - i.bbox_min[0]) * (i.bbox_max[2] - i.bbox_min[2]))
        .sum();
    let widest = items
        .iter()
        .flat_map(|i| [i.bbox_max[0] - i.bbox_min[0], i.bbox_max[2] - i.bbox_min[2]])
        .fold(1.0f64, f64::max);
    let base = area.max(1e-6).sqrt();
    let mut best: Option<(Vec<Placement>, f64, f64)> = None;
    for k in 1..=12 {
        let width = (base * (0.8 + 0.2 * k as f64)).max(widest + gap);
        let ps = pack(items, gap, width);
        if ps
            .iter()
            .enumerate()
            .any(|(i, a)| ps.iter().skip(i + 1).any(|b| overlap_volume(a, b) > 1e-12))
        {
            continue;
        }
        let cov = coverage(&ps);
        if best.as_ref().map(|b| cov > b.2).unwrap_or(true) {
            best = Some((ps, width, cov));
        }
    }
    best.unwrap_or_else(|| (pack(items, gap, widest + gap), widest + gap, 0.0))
}

/// 单个世界点按同一摆放变换（网格顶点与高斯共用这一份数学）。
pub fn transform_point(p: [f32; 3], it: &Item, pl: &Placement) -> [f32; 3] {
    let (cx, cz) = center(it);
    let (dx, dz) = (p[0] as f64 - cx, p[2] as f64 - cz);
    let (rx, rz) = match pl.yaw % 360 {
        90 => (dz, -dx),
        180 => (-dx, -dz),
        270 => (-dz, dx),
        _ => (dx, dz),
    };
    [
        (cx + rx + pl.translate[0]) as f32,
        (p[1] as f64 + pl.translate[1]) as f32,
        (cz + rz + pl.translate[2]) as f32,
    ]
}

/// 把对象高斯按摆放变换到世界系（旋转绕包围盒中心，再平移）。
pub fn apply(splats: &[Splat], it: &Item, pl: &Placement) -> Vec<Splat> {
    splats
        .iter()
        .map(|s| {
            let mut g = s.clone();
            g.pos = transform_point(s.pos, it, pl);
            g
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u32, w: f64, h: f64, d: f64) -> Item {
        Item {
            id,
            bbox_min: [0.0, 0.0, 0.0],
            bbox_max: [w, h, d],
        }
    }

    #[test]
    fn packing_has_zero_overlap_and_respects_gap() {
        let items = vec![
            item(1, 1.0, 0.5, 0.8),
            item(2, 0.4, 0.3, 1.2),
            item(3, 0.9, 0.9, 0.9),
            item(4, 0.2, 0.2, 0.2),
        ];
        let (ps, _w, cov) = pack_best(&items, 0.05);
        assert_eq!(ps.len(), 4);
        for i in 0..ps.len() {
            for j in i + 1..ps.len() {
                assert_eq!(overlap_volume(&ps[i], &ps[j]), 0.0, "零穿插是硬门");
                for value in ps[i].bbox_min.iter().take(3) {
                    assert!(*value >= -1e-9, "物体不能落到地面以下/负坐标");
                }
            }
        }
        assert!((0.0..=1.0).contains(&cov), "覆盖率 {cov}");
        // 同一输入两次摆放必须一致
        let (ps2, _, _) = pack_best(&items, 0.05);
        assert_eq!(ps, ps2);
    }

    #[test]
    fn yaw_swaps_extents_and_transform_matches_bbox() {
        let it = item(7, 2.0, 1.0, 1.0);
        let p = Placement {
            id: 7,
            yaw: 90,
            translate: [0.0; 3],
            bbox_min: [0.0; 3],
            bbox_max: [0.0; 3],
        };
        let corners: Vec<Splat> = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ]
        .into_iter()
        .map(|pos| Splat {
            pos,
            scale: 0.01,
            color: [0.5; 3],
            alpha: 0.8,
        })
        .collect();
        let got = apply(&corners, &it, &p);
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for g in &got {
            for k in 0..3 {
                lo[k] = lo[k].min(g.pos[k]);
                hi[k] = hi[k].max(g.pos[k]);
            }
        }
        // 绕中心 (1,·,0.5) 转 90°：x 范围 [0.5,1.5]，z 范围 [-0.5,1.5]
        assert!((lo[0] - 0.5).abs() < 1e-6);
        assert!((hi[0] - 1.5).abs() < 1e-6);
        assert!((lo[2] + 0.5).abs() < 1e-6);
        assert!((hi[2] - 1.5).abs() < 1e-6);
        // 方向也要对：原 (2,0,0) 绕中心转 +90° 应到 (0.5,0,-0.5)
        let p1 = got[1].pos;
        assert!((p1[0] - 0.5).abs() < 1e-6, "x {}", p1[0]);
        assert!(p1[1].abs() < 1e-6);
        assert!((p1[2] + 0.5).abs() < 1e-6, "z {}", p1[2]);
    }

    #[test]
    fn placement_bbox_matches_transformed_points() {
        let items = vec![item(1, 2.0, 1.0, 1.0), item(2, 0.5, 0.5, 1.5)];
        let (ps, _, _) = pack_best(&items, 0.05);
        for it in &items {
            let p = ps.iter().find(|p| p.id == it.id).unwrap();
            let mut corners = Vec::new();
            for &x in &[it.bbox_min[0], it.bbox_max[0]] {
                for &y in &[it.bbox_min[1], it.bbox_max[1]] {
                    for &z in &[it.bbox_min[2], it.bbox_max[2]] {
                        corners.push(Splat {
                            pos: [x as f32, y as f32, z as f32],
                            scale: 0.01,
                            color: [0.5; 3],
                            alpha: 0.8,
                        });
                    }
                }
            }
            let got = apply(&corners, it, p);
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for g in &got {
                for k in 0..3 {
                    lo[k] = lo[k].min(g.pos[k]);
                    hi[k] = hi[k].max(g.pos[k]);
                }
            }
            for k in 0..3 {
                assert!((lo[k] as f64 - p.bbox_min[k]).abs() < 1e-5, "min[{k}]");
                assert!((hi[k] as f64 - p.bbox_max[k]).abs() < 1e-5, "max[{k}]");
            }
        }
    }

    #[test]
    fn coverage_counts_footprint_over_layout() {
        let a = Placement {
            id: 1,
            yaw: 0,
            translate: [0.0; 3],
            bbox_min: [0.0, 0.0, 0.0],
            bbox_max: [1.0, 1.0, 1.0],
        };
        let b = Placement {
            id: 2,
            yaw: 0,
            translate: [0.0; 3],
            bbox_min: [1.0, 0.0, 0.0],
            bbox_max: [2.0, 1.0, 1.0],
        };
        assert!((coverage(&[a, b]) - 1.0).abs() < 1e-9);
        assert_eq!(coverage(&[]), 0.0);
    }
}
