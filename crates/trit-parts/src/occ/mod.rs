//! N2 学习式占据：每个体素取多视角特征 → 三值 MLP 判「实心/空」。
//!
//! - GT 实心来自场景定义（`objects.json` 的原语 + 位姿），不是点云：这样底面/背面也有标签；
//! - 特征只看**剪影**与**深度前后关系**，不看颜色（颜色捷径已踩过一次）；
//! - 训练用 STE（三值权重 + f32 缩放），与 `trit-learn` 同一套口径；
//! - 输出直接进 `surface_nets` —— 目的是把 N1 的开放壳补成闭合体。

use crate::fuse::View;

mod mlp;
pub use mlp::Mlp;

/// 场景原语（与渲染器一致：方块 size=1（part_cube 0.055）、球 r=0.7、
/// 柱 r=0.45/depth=1.2、环 major=0.5/minor=0.18；scale/rot 再作用）。
#[derive(Clone, Debug)]
pub struct Primitive {
    pub kind: String,
    pub center: [f32; 3],
    pub scale: [f32; 3],
    /// Blender 欧拉角（度），XYZ 顺序：R = Rz·Ry·Rx。
    pub rot: [f32; 3],
}

impl Primitive {
    /// 外接半径（米）：原语半尺寸 × 最大缩放。
    pub fn radius(&self) -> f32 {
        let m = self.scale.iter().cloned().fold(0.0f32, f32::max);
        match self.kind.as_str() {
            "part_cube" => 0.0275 * m,
            "cube" => 0.5 * m,
            "sphere" => 0.7 * m,
            "cylinder" => 0.6 * m,
            "torus" => 0.68 * m,
            _ => 0.5 * m,
        }
    }

    /// 世界点是否在实体内部。
    pub fn contains(&self, p: [f32; 3]) -> bool {
        let r = euler_zyx(self.rot);
        let d = [
            p[0] - self.center[0],
            p[1] - self.center[1],
            p[2] - self.center[2],
        ];
        // R^T d
        let q = [
            r[0][0] * d[0] + r[1][0] * d[1] + r[2][0] * d[2],
            r[0][1] * d[0] + r[1][1] * d[1] + r[2][1] * d[2],
            r[0][2] * d[0] + r[1][2] * d[1] + r[2][2] * d[2],
        ];
        let s = [
            q[0] / self.scale[0],
            q[1] / self.scale[1],
            q[2] / self.scale[2],
        ];
        match self.kind.as_str() {
            "part_cube" | "cube" => {
                let half = if self.kind == "cube" { 0.5 } else { 0.0275 };
                s[0].abs() <= half && s[1].abs() <= half && s[2].abs() <= half
            }
            "sphere" => (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt() <= 0.7,
            "cylinder" => (s[0] * s[0] + s[1] * s[1]).sqrt() <= 0.45 && s[2].abs() <= 0.6,
            "torus" => {
                let ring = (s[0] * s[0] + s[1] * s[1]).sqrt() - 0.5;
                (ring * ring + s[2] * s[2]).sqrt() <= 0.18
            }
            _ => false,
        }
    }
}

fn euler_zyx(deg: [f32; 3]) -> [[f32; 3]; 3] {
    let [rx, ry, rz] = [
        deg[0].to_radians(),
        deg[1].to_radians(),
        deg[2].to_radians(),
    ];
    let (sx, cx, sy, cy, sz, cz) = (rx.sin(), rx.cos(), ry.sin(), ry.cos(), rz.sin(), rz.cos());
    // Rz·Ry·Rx
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

/// 由原语包围盒建体素栅格（含 margin 格空边）。
pub fn grid_for(prims: &[Primitive], cell: f32, margin: usize) -> Option<crate::mesh::Grid> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in prims {
        let r = p.radius() * 1.05;
        for a in 0..3 {
            lo[a] = lo[a].min(p.center[a] - r);
            hi[a] = hi[a].max(p.center[a] + r);
        }
    }
    if !lo.iter().all(|v| v.is_finite()) {
        return None;
    }
    let dims =
        [0, 1, 2].map(|a| (((hi[a] - lo[a]) / cell).ceil() as usize + 1 + 2 * margin).max(1));
    let total = dims[0].saturating_mul(dims[1]).saturating_mul(dims[2]);
    if total > 60_000_000 {
        eprintln!("grid_for: 栅格过大 {dims:?}（{total} 体素），检查 scale/中心");
        return None;
    }
    Some(crate::mesh::Grid {
        occ: vec![0u8; dims[0] * dims[1] * dims[2]],
        nx: dims[0],
        ny: dims[1],
        nz: dims[2],
        origin: [
            lo[0] - margin as f32 * cell,
            lo[1] - margin as f32 * cell,
            lo[2] - margin as f32 * cell,
        ],
        cell,
    })
}

/// 原语实心标签（1 = 实心）。
pub fn solid(prims: &[Primitive], g: &crate::mesh::Grid) -> Vec<u8> {
    let mut out = vec![0u8; g.occ.len()];
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                let p = [
                    g.origin[0] + (x as f32 + 0.5) * g.cell,
                    g.origin[1] + (y as f32 + 0.5) * g.cell,
                    g.origin[2] + (z as f32 + 0.5) * g.cell,
                ];
                if prims.iter().any(|pr| pr.contains(p)) {
                    out[x + g.nx * (y + g.ny * z)] = 1;
                }
            }
        }
    }
    out
}

/// `View` 的 `ids` / `depth` 长度契约：两者都必须等于 `w×h`。
///
/// 契约本来该由 `.npy` 的 shape 提供，但构造 `View` 时 `depth` 的形状被丢掉了 ——
/// 两个文件尺寸不一致时，下面的 `v.ids[i]` / `v.depth[i]` 就会越界。
fn check_views(views: &[View]) {
    for v in views {
        assert!(
            v.ids.len() == v.w * v.h && v.depth.len() == v.w * v.h,
            "视角 ids/depth 长度必须等于 w×h（{}×{}）",
            v.w,
            v.h
        );
    }
}

/// 自监督实心标签（不需要物体定义）：
/// 任一视角里「投到背景像素」或「在观测表面前方 τ 之外」⇒ 自由空间；
/// 在所有视角都落在剪影内、且从未被判为自由 ⇒ 实心；从未进入任何视野 ⇒ 空（防远场全实）。
pub fn selfsup_solid(views: &[View], g: &crate::mesh::Grid, tau: f32) -> Vec<u8> {
    check_views(views);
    let mut out = vec![0u8; g.occ.len()];
    for z in 0..g.nz {
        for y in 0..g.ny {
            for x in 0..g.nx {
                let p = [
                    g.origin[0] + (x as f32 + 0.5) * g.cell,
                    g.origin[1] + (y as f32 + 0.5) * g.cell,
                    g.origin[2] + (z as f32 + 0.5) * g.cell,
                ];
                let mut seen = false;
                let mut free = false;
                for v in views {
                    let Some((u, vv, d)) = v.cam.project(p) else {
                        continue;
                    };
                    if u < 0.0 || vv < 0.0 || u >= v.w as f64 || vv >= v.h as f64 {
                        continue;
                    }
                    seen = true;
                    let i = vv as usize * v.w + u as usize;
                    if v.ids[i] == 0 {
                        free = true; // 该视角射线打到空处 ⇒ 这个体素是自由空间
                        break;
                    }
                    if d < v.depth[i] as f64 - f64::from(tau) {
                        free = true; // 在观测表面前方 ⇒ 自由空间
                        break;
                    }
                }
                out[x + g.nx * (y + g.ny * z)] = (seen && !free) as u8;
            }
        }
    }
    out
}

/// 多视角特征维度：每视角 3 个（剪影、深度差、有效），外加 1 个体素中心偏置项。
pub const FPER: usize = 3;

/// 一个体素的特征：各视角 [在剪影内, clip((d_vox − d_obs)/0.2, −1, 1), 有效] + 常数 1。
pub fn features(views: &[View], p: [f32; 3]) -> Vec<f32> {
    check_views(views);
    let mut f = Vec::with_capacity(views.len() * FPER + 1);
    for v in views {
        match v.cam.project(p) {
            Some((u, vv, d)) => {
                if u < 0.0 || vv < 0.0 || u >= v.w as f64 || vv >= v.h as f64 {
                    f.extend_from_slice(&[0.0, 0.0, 0.0]);
                    continue;
                }
                let i = vv as usize * v.w + u as usize;
                let inside = if v.ids[i] != 0 { 1.0 } else { 0.0 };
                let delta = if v.ids[i] == 0 {
                    0.0
                } else {
                    let dd = (d - v.depth[i] as f64) / 0.2;
                    dd.clamp(-1.0, 1.0) as f32
                };
                f.extend_from_slice(&[inside, delta, 1.0]);
            }
            None => f.extend_from_slice(&[0.0, 0.0, 0.0]),
        }
    }
    f.push(1.0);
    f
}

/// 学习率线性衰减：`step ∈ [0, steps)`，从 `lr` 降到 `lr_final`（`None` = 恒定）。
pub fn lr_at(step: usize, steps: usize, lr: f32, lr_final: Option<f32>) -> f32 {
    match lr_final {
        Some(end) if steps > 1 => {
            let frac = step as f32 / (steps - 1) as f32;
            lr + (end - lr) * frac
        }
        _ => lr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lr_decays_linearly_to_final() {
        assert_eq!(lr_at(0, 10, 0.05, None), 0.05);
        assert!((lr_at(0, 10, 0.05, Some(0.005)) - 0.05).abs() < 1e-9);
        assert!((lr_at(9, 10, 0.05, Some(0.005)) - 0.005).abs() < 1e-9);
        assert!((lr_at(4, 9, 0.05, Some(0.005)) - 0.0275).abs() < 1e-6);
        // 单步/空计划不除零
        assert_eq!(lr_at(0, 1, 0.05, Some(0.005)), 0.05);
        assert_eq!(lr_at(0, 0, 0.05, Some(0.005)), 0.05);
    }

    /// 偶数种子与下一个奇数种子不能撞成同一初始化（旧 `seed | 1` 的 bug）。
    #[test]
    fn even_and_odd_seeds_init_differently() {
        let a = Mlp::new(6, 4, 2).unwrap();
        let b = Mlp::new(6, 4, 3).unwrap();
        assert_ne!(a.w1, b.w1, "seed 2 与 3 的初始化相同");
        assert_ne!(Mlp::new(6, 4, 4).unwrap().w1, Mlp::new(6, 4, 5).unwrap().w1);
        // 奇数种子仍确定性（既有跑法可复现）
        assert_eq!(Mlp::new(6, 4, 3).unwrap().w1, b.w1);
    }

    #[test]
    fn primitives_containment() {
        let cube = Primitive {
            kind: "part_cube".into(),
            center: [0.0; 3],
            scale: [1.0; 3],
            rot: [0.0; 3],
        };
        assert!(cube.contains([0.0, 0.0, 0.0]));
        assert!(!cube.contains([0.1, 0.0, 0.0]), "part_cube 半边长 2.75cm");
        let sph = Primitive {
            kind: "sphere".into(),
            center: [1.0, 0.0, 0.0],
            scale: [1.0; 3],
            rot: [0.0; 3],
        };
        assert!(sph.contains([1.5, 0.0, 0.0]));
        assert!(!sph.contains([1.8, 0.0, 0.0]));
        let cyl = Primitive {
            kind: "cylinder".into(),
            center: [0.0; 3],
            scale: [1.0; 3],
            rot: [0.0, 0.0, 0.0],
        };
        assert!(cyl.contains([0.0, 0.0, 0.55]));
        assert!(!cyl.contains([0.0, 0.0, 0.7]));
        // 绕 z 转 90°：柱轴变成 y 轴
        let cyl_rot = Primitive {
            kind: "cylinder".into(),
            center: [0.0; 3],
            scale: [1.0; 3],
            rot: [90.0, 0.0, 0.0],
        };
        assert!(cyl_rot.contains([0.0, 0.55, 0.0]), "绕 x 转 90° 后轴线沿 y");
    }

    #[test]
    fn mlp_learns_separable_toy() {
        // 两维可分数据：x0 > 0 ⇒ 1
        let mut m = Mlp::new(2, 8, 7).unwrap();
        let data = [
            ([1.0f32, 0.0], 1.0f32),
            ([-1.0, 0.0], 0.0),
            ([0.5, 0.2], 1.0),
            ([-0.5, -0.2], 0.0),
        ];
        for _ in 0..300 {
            for (x, y) in data {
                m.train_step(&x, y, 0.1);
            }
        }
        for (x, y) in data {
            let (z, _) = m.forward(&x);
            let p = 1.0 / (1.0 + (-z).exp());
            assert!((p - y).abs() < 0.2, "x={x:?} p={p}");
        }
    }

    #[test]
    fn selfsup_carves_free_space_only() {
        // 100×100 视野，中央 20×20 是前景、深度 2 m；tau=0.05
        let (w, h) = (100usize, 100usize);
        let mut ids = vec![0u32; w * h];
        for y in 40..60 {
            for x in 40..60 {
                ids[y * w + x] = 1;
            }
        }
        let view = View {
            ids,
            depth: vec![2.0; w * h],
            w,
            h,
            cam: crate::geo::Camera {
                k: [[100.0, 0.0, 50.0], [0.0, 100.0, 50.0], [0.0, 0.0, 1.0]],
                t_cw: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                    [0.0, 0.0, 0.0, 1.0],
                ],
            },
        };
        let (nx, ny, nz) = (100usize, 100usize, 190usize);
        let g = crate::mesh::Grid {
            occ: vec![0u8; nx * ny * nz],
            nx,
            ny,
            nz,
            origin: [-0.5, -0.5, -2.6],
            cell: 0.01,
        };
        let lab = selfsup_solid(std::slice::from_ref(&view), &g, 0.05);
        let at = |p: [f32; 3]| {
            let c = [0, 1, 2].map(|a| ((p[a] - g.origin[a]) / g.cell).round() as usize);
            lab[c[0] + nx * (c[1] + ny * c[2])]
        };
        assert_eq!(at([0.0, 0.0, -1.0]), 0, "表面前方 = 自由");
        assert_eq!(at([0.0, 0.0, -2.0]), 1, "贴着表面 = 实心");
        assert_eq!(at([0.0, 0.0, -2.5]), 1, "剪影内、被遮挡 ⇒ 实心（视觉外壳）");
        assert_eq!(at([0.3, 0.0, -2.5]), 0, "剪影外 ⇒ 自由");
        assert_eq!(at([0.45, 0.45, -0.9]), 0, "视野外（画幅之外）⇒ 空");
    }

    #[test]
    fn features_shape_and_outside_pixels() {
        // 相机在原点看 −z；体素放到画面外应得到 [0,0,0]
        let cam = crate::geo::Camera {
            k: [[100.0, 0.0, 50.0], [0.0, 100.0, 50.0], [0.0, 0.0, 1.0]],
            t_cw: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        let view = View {
            ids: vec![1; 100 * 100],
            depth: vec![2.0; 100 * 100],
            w: 100,
            h: 100,
            cam,
        };
        let inside = features(std::slice::from_ref(&view), [0.0, 0.0, -2.0]);
        assert_eq!(inside.len(), FPER + 1);
        assert!((inside[0] - 1.0).abs() < 1e-6, "应落在剪影内");
        let outside = features(std::slice::from_ref(&view), [10.0, 0.0, -2.0]);
        assert!((outside[0]).abs() < 1e-6, "画面外应为 0");
    }

    /// `depth` 与 `seg` 尺寸不一致时必须在索引前拒绝 —— 构造点丢掉了 depth 的 shape。
    #[test]
    #[should_panic(expected = "长度必须等于 w×h")]
    fn view_with_short_depth_is_rejected() {
        let cam = crate::geo::Camera {
            k: [[0.0; 3]; 3],
            t_cw: [[0.0; 4]; 4],
        };
        let view = View {
            ids: vec![1; 4],
            depth: vec![0.0; 3],
            w: 2,
            h: 2,
            cam,
        };
        features(std::slice::from_ref(&view), [0.0; 3]);
    }
}
