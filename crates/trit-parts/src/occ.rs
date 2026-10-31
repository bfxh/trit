//! N2 学习式占据：每个体素取多视角特征 → 三值 MLP 判「实心/空」。
//!
//! - GT 实心来自场景定义（`objects.json` 的原语 + 位姿），不是点云：这样底面/背面也有标签；
//! - 特征只看**剪影**与**深度前后关系**，不看颜色（颜色捷径已踩过一次）；
//! - 训练用 STE（三值权重 + f32 缩放），与 `trit-learn` 同一套口径；
//! - 输出直接进 `surface_nets` —— 目的是把 N1 的开放壳补成闭合体。

use crate::fuse::View;

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
        eprintln!(
            "grid_for: 栅格过大 {:?}（{} 体素），检查 scale/中心",
            dims, total
        );
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

/// 多视角特征维度：每视角 3 个（剪影、深度差、有效），外加 1 个体素中心偏置项。
pub const FPER: usize = 3;

/// 一个体素的特征：各视角 [在剪影内, clip((d_vox − d_obs)/0.2, −1, 1), 有效] + 常数 1。
pub fn features(views: &[View], p: [f32; 3]) -> Vec<f32> {
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

/// 两层三值 MLP（隐藏层 ReLU，输出 logit），STE 反向。
pub struct Mlp {
    pub nin: usize,
    pub nhid: usize,
    /// 实际值（三值 × 每层缩放）；训练时直接更新，量化在 `q()` 里取。
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: f32,
}

/// 三值化：有效权重 ∈ {−1, 0, +1}（每层缩放交给输出层的 f32 权重吸收，
/// 否则 0.02 量级的隐藏激活会把 logit 动态范围压死）。
fn quant(v: f32) -> f32 {
    v.round().clamp(-1.0, 1.0)
}

impl Mlp {
    pub fn new(nin: usize, nhid: usize, seed: u64) -> Self {
        let mut r = seed | 1;
        let mut next = || {
            r ^= r << 13;
            r ^= r >> 7;
            r ^= r << 17;
            ((r % 2001) as f32 - 1000.0) / 2500.0
        };
        let n1 = nin * nhid;
        let n2 = nhid;
        Mlp {
            nin,
            nhid,
            w1: (0..n1).map(|_| next() * 5.0).collect(),
            b1: vec![0.0; nhid],
            w2: (0..n2).map(|_| next()).collect(),
            b2: 0.0,
        }
    }

    /// 前向：返回 (logit, 隐藏层)。
    pub fn forward(&self, x: &[f32]) -> (f32, Vec<f32>) {
        let mut h = vec![0.0f32; self.nhid];
        for (j, hj) in h.iter_mut().enumerate() {
            let mut s = self.b1[j];
            for (i, xi) in x.iter().enumerate().take(self.nin) {
                s += quant(self.w1[i * self.nhid + j]) * xi;
            }
            *hj = s.max(0.0);
        }
        let mut z = self.b2;
        // 输出层保持 f32（低比特网络的通行做法：首/末层不全三值，否则 logit 动态范围被压死）
        for (j, hj) in h.iter().enumerate() {
            z += self.w2[j] * hj;
        }
        (z, h)
    }

    /// 单样本 BCE + STE 梯度更新（SGD）。
    pub fn train_step(&mut self, x: &[f32], y: f32, lr: f32) -> f32 {
        let (z, h) = self.forward(x);
        let p = 1.0 / (1.0 + (-z).exp());
        let eps = 1e-7;
        let loss = -(y * (p + eps).ln() + (1.0 - y) * (1.0 - p + eps).ln());
        let dz = p - y;
        // 输出层
        for (w, hj) in self.w2.iter_mut().zip(h.iter()) {
            *w -= lr * dz * *hj;
        }
        self.b2 -= lr * dz;
        // 隐藏层（STE：梯度穿过量化）
        for (j, hj) in h.iter().enumerate() {
            if *hj <= 0.0 {
                continue;
            }
            let dh = dz * self.w2[j];
            for (i, xi) in x.iter().enumerate().take(self.nin) {
                self.w1[i * self.nhid + j] -= lr * dh * xi;
            }
            self.b1[j] -= lr * dh;
        }
        loss
    }

    /// 权重三值化后的稀疏度（0 的占比），用于报告。
    pub fn sparsity(&self) -> f32 {
        let z = self.w1.iter().filter(|v| quant(**v) == 0.0).count();
        z as f32 / self.w1.len().max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut m = Mlp::new(2, 8, 7);
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
}
