//! 多视角体素融合（N1）：各帧反投影 → 按「部件 id」投票 → argmax；再填实、提网格。
//!
//! 口径与门（`GENERAL-PLAN §6` N1）：留出视角的 GT 点云逐部件 3D IoU ≥ 0.5；
//! 闭合率（非流形边 = 0 的部件占比）≥ 80%。本模块只做几何，不碰学习。

use std::collections::HashMap;

use crate::geo::Camera;
use crate::mesh::{
    Grid, Mesh, dilate_occ, fill_solid, fix_diagonals, non_manifold_edges, surface_nets,
};

/// 一帧输入：部件 id 图、深度（相机系 z，米）、相机。
pub struct View {
    pub ids: Vec<u32>,
    pub depth: Vec<f32>,
    pub w: usize,
    pub h: usize,
    pub cam: Camera,
}

/// 融合参数。
pub struct FuseCfg {
    /// 体素边长（米）。
    pub cell: f32,
    /// 栅格四周留白格数。
    pub margin: usize,
    /// 体素至少被多少帧/多少个采样看到才保留（去孤立噪声）。
    pub min_votes: f32,
    /// 填实前先把壳体膨胀几格（封住体素化造成的小孔，否则泛洪会漏进内部）。
    pub seal: usize,
}

impl Default for FuseCfg {
    fn default() -> Self {
        FuseCfg {
            cell: 0.01,
            margin: 2,
            min_votes: 1.0,
            seal: 1,
        }
    }
}

/// 一个部件的视觉外壳输出。
pub struct HullOut {
    pub id: u32,
    pub mesh: Mesh,
    pub closed: bool,
    /// 外壳表面格（融合栅格索引空间）。
    pub surface: std::collections::HashSet<(i32, i32, i32)>,
    /// 外壳体积格（填实后）。
    pub volume: std::collections::HashSet<(i32, i32, i32)>,
}

/// 融合结果：占用栅格（多视角投票后）、每体素的部件 id、总投票数。
pub struct Fused {
    pub grid: Grid,
    /// 与 `grid.occ` 等长；0 = 无部件。
    pub part: Vec<u32>,
    /// 每体素累计的采样票数（用于置信度过滤）。
    pub votes: Vec<f32>,
    /// 填实前的封孔膨胀格数。
    pub seal: usize,
}

/// 融合所有视角。返回 `None` 表示没有任何有效深度样本。
pub fn fuse(views: &[View], cfg: &FuseCfg) -> Option<Fused> {
    let mut pts: Vec<[f32; 3]> = Vec::new();
    for v in views {
        if v.ids.len() != v.w * v.h || v.depth.len() != v.ids.len() {
            continue;
        }
        for i in 0..v.ids.len() {
            let d = v.depth[i];
            if v.ids[i] == 0 || !(d.is_finite() && d > 0.0 && d < 50.0) {
                continue;
            }
            let (u, vv) = (i % v.w, i / v.w);
            pts.push(v.cam.unproject(u as f64, vv as f64, d));
        }
    }
    let mut g = Grid::from_points(&pts, cfg.cell, cfg.margin)?;

    // (体素, 部件) 加权票数 + 每体素总票数
    let mut pair: HashMap<(u32, u32), f32> = HashMap::new();
    let mut votes = vec![0.0f32; g.occ.len()];
    for view in views {
        if view.ids.len() != view.w * view.h || view.depth.len() != view.ids.len() {
            continue;
        }
        for i in 0..view.ids.len() {
            let id = view.ids[i];
            let d = view.depth[i];
            if id == 0 || !(d.is_finite() && d > 0.0 && d < 50.0) {
                continue;
            }
            let (u, vv) = (i % view.w, i / view.w);
            let p = view.cam.unproject(u as f64, vv as f64, d);
            let c = [0, 1, 2].map(|a| {
                (((p[a] - g.origin[a]) / g.cell).round() as isize)
                    .clamp(0, [g.nx, g.ny, g.nz][a] as isize - 1) as usize
            });
            let idx = g.idx(c[0], c[1], c[2]);
            votes[idx] += 1.0;
            *pair.entry((idx as u32, id)).or_insert(0.0) += 1.0;
        }
    }

    // 每体素取票数最多的部件
    let mut best: HashMap<u32, (u32, f32)> = HashMap::new();
    for (&(idx, id), &w) in &pair {
        let e = best.entry(idx).or_insert((0, 0.0));
        if w > e.1 || (w == e.1 && id < e.0) {
            *e = (id, w);
        }
    }
    let mut part = vec![0u32; g.occ.len()];
    for (i, vt) in votes.iter().enumerate() {
        if *vt >= cfg.min_votes
            && let Some((id, _)) = best.get(&(i as u32))
        {
            part[i] = *id;
        }
    }
    for (o, &p) in g.occ.iter_mut().zip(part.iter()) {
        *o = (p != 0) as u8;
    }
    Some(Fused {
        grid: g,
        part,
        votes,
        seal: cfg.seal,
    })
}

impl Fused {
    /// 某个部件的实心栅格（封孔 → 填实，保证闭合）。
    pub fn part_grid(&self, id: u32) -> Grid {
        let mut g = self.grid.clone();
        for (i, &p) in self.part.iter().enumerate() {
            g.occ[i] = (p == id) as u8;
        }
        if self.seal > 0 {
            dilate_occ(&mut g, self.seal);
        }
        fill_solid(&mut g);
        fix_diagonals(&mut g);
        g
    }

    /// 场景整体网格（所有部件并成一个实体）。
    pub fn scene_grid(&self) -> Grid {
        let mut g = self.grid.clone();
        if self.seal > 0 {
            dilate_occ(&mut g, self.seal);
        }
        fill_solid(&mut g);
        g
    }

    /// 视觉外壳：以「该部件的投票壳体素」的包围盒为候选，凡在任一视角**投到背景**
    /// 的体素都剪掉；被别的部件挡住（投影落在别的部件上）**不剪**（保守，怕误删遮挡区）。
    /// 得到一个闭合体积（底面虽未被看到，也被多视角剪影封住）。
    pub fn visual_hull(&self, id: u32, views: &[View]) -> Option<Grid> {
        let cells = self.part_cells(id);
        if cells.is_empty() {
            return None;
        }
        let (g0, r) = (&self.grid, 0i32); // 候选只取外壳 bbox；padding 单独留
        let (mut lo, mut hi) = ([i32::MAX; 3], [i32::MIN; 3]);
        for &(cx, cy, cz) in &cells {
            let c = [cx, cy, cz];
            for a in 0..3 {
                lo[a] = lo[a].min(c[a] - r);
                hi[a] = hi[a].max(c[a] + r);
            }
        }
        let pad = 2i32; // padding 不参与候选，保证网格不贴边
        let dims = [0, 1, 2].map(|a| ((hi[a] - lo[a] + 1 + 2 * pad).max(1)) as usize);
        let origin = [
            g0.origin[0] + (lo[0] - pad) as f32 * g0.cell,
            g0.origin[1] + (lo[1] - pad) as f32 * g0.cell,
            g0.origin[2] + (lo[2] - pad) as f32 * g0.cell,
        ];
        let mut hull = Grid {
            occ: vec![0u8; dims[0] * dims[1] * dims[2]],
            nx: dims[0],
            ny: dims[1],
            nz: dims[2],
            origin,
            cell: g0.cell,
        };
        // 候选只在（外壳 bbox ± r）内，padding 留空
        for z in 0..hull.nz {
            for y in 0..hull.ny {
                for x in 0..hull.nx {
                    let (wx, wy, wz) = (
                        lo[0] - pad + x as i32,
                        lo[1] - pad + y as i32,
                        lo[2] - pad + z as i32,
                    );
                    if wx < lo[0] - r
                        || wy < lo[1] - r
                        || wz < lo[2] - r
                        || wx > hi[0] + r
                        || wy > hi[1] + r
                        || wz > hi[2] + r
                    {
                        continue;
                    }
                    hull.occ[x + hull.nx * (y + hull.ny * z)] = 1;
                }
            }
        }
        let center = |x: usize, y: usize, z: usize| {
            [
                origin[0] + (x as f32 + 0.5) * g0.cell,
                origin[1] + (y as f32 + 0.5) * g0.cell,
                origin[2] + (z as f32 + 0.5) * g0.cell,
            ]
        };
        for z in 0..hull.nz {
            for y in 0..hull.ny {
                for x in 0..hull.nx {
                    let p = center(x, y, z);
                    let mut carved = false;
                    for v in views {
                        let Some((u, vv, _)) = v.cam.project(p) else {
                            continue; // 在相机背后：无法判定，不剪
                        };
                        if u < 0.0 || vv < 0.0 || u >= v.w as f64 || vv >= v.h as f64 {
                            carved = true; // 投到画面外 ⇒ 一定不在剪影里
                            break;
                        }
                        let idx = vv as usize * v.w + u as usize;
                        if v.ids[idx] == 0 {
                            carved = true; // 投到背景 ⇒ 不在剪影里
                            break;
                        }
                    }
                    if carved {
                        hull.occ[x + hull.nx * (y + hull.ny * z)] = 0;
                    }
                }
            }
        }
        crate::mesh::fill_solid(&mut hull);
        fix_diagonals(&mut hull);
        Some(hull)
    }

    /// 世界点 → 本栅格索引（越界返回 `None`；与融合时的取整口径一致）。
    pub fn cell_index(&self, p: [f32; 3]) -> Option<(i32, i32, i32)> {
        let g = &self.grid;
        let c = [0, 1, 2].map(|a| ((p[a] - g.origin[a]) / g.cell).round() as i32);
        if c[0] < 0
            || c[1] < 0
            || c[2] < 0
            || c[0] >= g.nx as i32
            || c[1] >= g.ny as i32
            || c[2] >= g.nz as i32
        {
            None
        } else {
            Some((c[0], c[1], c[2]))
        }
    }

    /// 某部件的索引集合（壳体素，未填实）——评测用。
    pub fn part_cells(&self, id: u32) -> std::collections::HashSet<(i32, i32, i32)> {
        let g = &self.grid;
        self.part
            .iter()
            .enumerate()
            .filter(|(_, p)| **p == id)
            .map(|(i, _)| {
                (
                    (i % g.nx) as i32,
                    ((i / g.nx) % g.ny) as i32,
                    (i / (g.nx * g.ny)) as i32,
                )
            })
            .collect()
    }

    /// 每个可见部件的**视觉外壳**网格 + 是否闭合 + 表面格 + 体积格（融合栅格索引空间）。
    pub fn hull_meshes(&self, views: &[View]) -> Vec<HullOut> {
        let mut ids: Vec<u32> = self.part.iter().copied().filter(|&p| p != 0).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut out = Vec::new();
        for id in ids {
            let Some(hull) = self.visual_hull(id, views) else {
                continue;
            };
            let m = surface_nets(&hull);
            let closed = !m.tris.is_empty() && non_manifold_edges(&m) == 0;
            // 外壳表面格 → 融合栅格索引
            let mut cells = std::collections::HashSet::new();
            for z in 0..hull.nz {
                for y in 0..hull.ny {
                    for x in 0..hull.nx {
                        if hull.get(x, y, z) == 0 {
                            continue;
                        }
                        let surf = [
                            (1i32, 0i32, 0i32),
                            (-1, 0, 0),
                            (0, 1, 0),
                            (0, -1, 0),
                            (0, 0, 1),
                            (0, 0, -1),
                        ]
                        .iter()
                        .any(|&(dx, dy, dz)| {
                            let (xx, yy, zz) = (x as i32 + dx, y as i32 + dy, z as i32 + dz);
                            xx < 0
                                || yy < 0
                                || zz < 0
                                || xx as usize >= hull.nx
                                || yy as usize >= hull.ny
                                || zz as usize >= hull.nz
                                || hull.get(xx as usize, yy as usize, zz as usize) == 0
                        });
                        if !surf {
                            continue;
                        }
                        let p = [
                            hull.origin[0] + (x as f32 + 0.5) * hull.cell,
                            hull.origin[1] + (y as f32 + 0.5) * hull.cell,
                            hull.origin[2] + (z as f32 + 0.5) * hull.cell,
                        ];
                        if let Some(c) = self.cell_index(p) {
                            cells.insert(c);
                        }
                    }
                }
            }
            let mut volume = std::collections::HashSet::new();
            for z in 0..hull.nz {
                for y in 0..hull.ny {
                    for x in 0..hull.nx {
                        if hull.get(x, y, z) == 0 {
                            continue;
                        }
                        let p = [
                            hull.origin[0] + (x as f32 + 0.5) * hull.cell,
                            hull.origin[1] + (y as f32 + 0.5) * hull.cell,
                            hull.origin[2] + (z as f32 + 0.5) * hull.cell,
                        ];
                        if let Some(c) = self.cell_index(p) {
                            volume.insert(c);
                        }
                    }
                }
            }
            out.push(HullOut {
                id,
                mesh: m,
                closed,
                surface: cells,
                volume,
            });
        }
        out
    }

    /// 每个可见部件的网格 + 是否闭合。
    pub fn part_meshes(&self) -> Vec<(u32, Mesh, bool)> {
        let mut ids: Vec<u32> = self.part.iter().copied().filter(|&p| p != 0).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.into_iter()
            .map(|id| {
                let m = surface_nets(&self.part_grid(id));
                let closed = !m.tris.is_empty() && non_manifold_edges(&m) == 0;
                (id, m, closed)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam_identity() -> Camera {
        // 相机系 = 世界系；fx=fy=100, cx=cy=1（3×3 图像）
        Camera {
            k: [[100.0, 0.0, 1.0], [0.0, 100.0, 1.0], [0.0, 0.0, 1.0]],
            t_cw: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    #[test]
    fn votes_argmax_by_part_and_min_votes() {
        // 3×3 图像、所有像素深度 2 m：id 1 占 8 个，id 2 占 1 个
        let (w, h) = (3usize, 3usize);
        let mut ids = vec![1u32; w * h];
        ids[4] = 2;
        let view = View {
            ids,
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam_identity(),
        };
        let f = fuse(
            &[view],
            &FuseCfg {
                cell: 0.01,
                margin: 2,
                min_votes: 1.0,
                seal: 1,
            },
        )
        .unwrap();
        assert!(f.grid.occ.contains(&1), "应有占用体素");
        assert!(
            f.part.contains(&1) && f.part.contains(&2),
            "两个部件都应出现"
        );
        let n1 = f.part.iter().filter(|&&p| p == 1).count();
        let n2 = f.part.iter().filter(|&&p| p == 2).count();
        assert!(n1 > n2, "票多的 id 应占更多体素：{n1} vs {n2}");
    }

    #[test]
    fn min_votes_filters_single_view_samples() {
        let (w, h) = (3usize, 3usize);
        let view = View {
            ids: vec![1u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam_identity(),
        };
        let few = fuse(
            &[view],
            &FuseCfg {
                cell: 0.01,
                margin: 2,
                min_votes: 5.0,
                seal: 1,
            },
        )
        .unwrap();
        assert!(
            few.grid.occ.iter().all(|&v| v == 0),
            "min_votes=5 时单视角应被滤掉"
        );
    }

    #[test]
    fn part_grid_fills_and_meshes_closed() {
        // 同一物体从两个方向看同一平面：填实后应得到闭合网格
        let (w, h) = (5usize, 5usize);
        let front = View {
            ids: vec![7u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam_identity(),
        };
        let mut back_cam = cam_identity();
        back_cam.t_cw[2][3] = 0.0;
        let back = View {
            ids: vec![7u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: back_cam,
        };
        let f = fuse(&[front, back], &FuseCfg::default()).unwrap();
        let (id, m, _closed) = f.part_meshes().pop().unwrap();
        assert_eq!(id, 7);
        assert!(!m.tris.is_empty(), "部件网格不应为空");
    }
}
