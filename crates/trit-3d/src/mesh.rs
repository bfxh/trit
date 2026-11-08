//! 真实三角网格 → 三值体素 / 高斯目标（M0 独立实现）。
//!
//! 不依赖外部 GPL 代码：这里只做两件事——
//! 1. 解析 torus 生成非平凡、封闭的测试网格；
//! 2. 射线奇偶体素化（给编码器输入）+ 面积加权表面采样（给高斯头目标）。
//!
//! 采样是确定性的（分层 + 低差异，无随机数），同一网格必得同一组高斯/点。

use std::f32::consts::PI;

use crate::VoxelGrid;
use crate::gaussian::{Gaussian, ShDegree, target_sh};

/// 三角网格。
#[derive(Clone, Debug, PartialEq)]
pub struct TriMesh {
    /// 顶点。
    pub verts: Vec<[f32; 3]>,
    /// 三角形索引。
    pub tris: Vec<[u32; 3]>,
}

impl TriMesh {
    /// 圆环：主半径 `major`、管半径 `minor`，`seg_u × seg_v` 分段。
    pub fn torus(major: f32, minor: f32, seg_u: usize, seg_v: usize) -> Self {
        assert!(seg_u >= 3 && seg_v >= 3, "分段数至少 3");
        let mut verts = Vec::with_capacity(seg_u * seg_v);
        for i in 0..seg_u {
            let u = 2.0 * PI * i as f32 / seg_u as f32;
            for j in 0..seg_v {
                let v = 2.0 * PI * j as f32 / seg_v as f32;
                let r = major + minor * v.cos();
                verts.push([r * u.cos(), r * u.sin(), minor * v.sin()]);
            }
        }
        let mut tris = Vec::with_capacity(seg_u * seg_v * 2);
        for i in 0..seg_u {
            let i1 = (i + 1) % seg_u;
            for j in 0..seg_v {
                let j1 = (j + 1) % seg_v;
                let a = (i * seg_v + j) as u32;
                let b = (i1 * seg_v + j) as u32;
                let c = (i1 * seg_v + j1) as u32;
                let d = (i * seg_v + j1) as u32;
                tris.push([a, b, c]);
                tris.push([a, c, d]);
            }
        }
        Self { verts, tris }
    }


    /// 读 OBJ（只取 `v` / `f`，忽略 `vt`/`vn`/`usemtl`）；面按扇形三角化。
    pub fn from_obj(path: &std::path::Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("读不了 {}：{e}", path.display()))?;
        let mut verts = Vec::new();
        let mut tris = Vec::new();
        for (ln, line) in text.lines().enumerate() {
            let mut it = line.split_whitespace();
            match it.next() {
                Some("v") => {
                    let mut xyz = [0.0f32; 3];
                    for xyz_k in &mut xyz {
                        let tok = it
                            .next()
                            .ok_or_else(|| format!("第 {} 行 v 缺分量", ln + 1))?;
                        *xyz_k = tok
                            .parse()
                            .map_err(|_| format!("第 {} 行 v 不是数字：{tok}", ln + 1))?;
                    }
                    verts.push(xyz);
                }
                Some("f") => {
                    let mut idx = Vec::new();
                    for tok in it {
                        let head = tok.split('/').next().unwrap_or(tok);
                        let raw: i64 = head
                            .parse()
                            .map_err(|_| format!("第 {} 行面索引不是整数：{head}", ln + 1))?;
                        let n = verts.len() as i64;
                        let i = if raw > 0 {
                            raw - 1
                        } else if raw < 0 {
                            n + raw
                        } else {
                            return Err(format!("第 {} 行面索引 0 非法", ln + 1));
                        };
                        if i < 0 || i >= n {
                            return Err(format!("第 {} 行面索引越界：{raw}（顶点 {n}）", ln + 1));
                        }
                        idx.push(i as u32);
                    }
                    for k in 1..idx.len().saturating_sub(1) {
                        tris.push([idx[0], idx[k], idx[k + 1]]);
                    }
                }
                _ => {}
            }
        }
        if verts.is_empty() || tris.is_empty() {
            return Err("OBJ 没有顶点或面".into());
        }
        Ok(Self { verts, tris })
    }

    /// 表面体素化：三角形顶点 + 质心落到体素标 `+1`，其余 `-1`（O(F)，不做射线填充）。
    pub fn voxelize_surface(&self, size: usize) -> VoxelGrid {
        assert!(size >= 2, "体素边长至少 2");
        let mut g = VoxelGrid::new(size);
        let denom = (size - 1) as f32;
        let put = |g: &mut VoxelGrid, p: [f32; 3]| {
            let mut ix = [0usize; 3];
            for k in 0..3 {
                ix[k] = (((p[k] + 1.0) * 0.5 * denom).round() as isize)
                    .clamp(0, size as isize - 1) as usize;
            }
            g.set(ix[0], ix[1], ix[2], 1);
        };
        for t in &self.tris {
            let a = self.verts[t[0] as usize];
            let b = self.verts[t[1] as usize];
            let c = self.verts[t[2] as usize];
            put(&mut g, a);
            put(&mut g, b);
            put(&mut g, c);
            put(&mut g, [
                (a[0] + b[0] + c[0]) / 3.0,
                (a[1] + b[1] + c[1]) / 3.0,
                (a[2] + b[2] + c[2]) / 3.0,
            ]);
        }
        g
    }
    /// AABB。
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for v in &self.verts {
            for k in 0..3 {
                min[k] = min[k].min(v[k]);
                max[k] = max[k].max(v[k]);
            }
        }
        (min, max)
    }

    /// 居中并缩放到 `[-0.8, 0.8]³`。
    pub fn normalize(&mut self) {
        let (min, max) = self.bounds();
        let center = [
            (min[0] + max[0]) * 0.5,
            (min[1] + max[1]) * 0.5,
            (min[2] + max[2]) * 0.5,
        ];
        let half = ((max[0] - min[0]).max(max[1] - min[1]).max(max[2] - min[2]) * 0.5).max(1e-6);
        let s = 0.8 / half;
        for v in &mut self.verts {
            for k in 0..3 {
                v[k] = (v[k] - center[k]) * s;
            }
        }
    }

    /// 三角形面积。
    pub fn tri_area(&self, t: [u32; 3]) -> f32 {
        let a = self.verts[t[0] as usize];
        let b = self.verts[t[1] as usize];
        let c = self.verts[t[2] as usize];
        let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        0.5 * (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt()
    }

    /// 射线奇偶体素化：体素中心在 +x 射线与网格交点数为奇数 ⇒ 内部 `+1`。
    pub fn voxelize(&self, size: usize) -> VoxelGrid {
        assert!(size >= 2, "体素边长至少 2");
        let mut g = VoxelGrid::new(size);
        let denom = (size - 1) as f32;
        for z in 0..size {
            let pz = z as f32 / denom * 2.0 - 1.0;
            for y in 0..size {
                let py = y as f32 / denom * 2.0 - 1.0;
                for x in 0..size {
                    let px = x as f32 / denom * 2.0 - 1.0;
                    let mut hits = 0usize;
                    for t in &self.tris {
                        let a = self.verts[t[0] as usize];
                        let b = self.verts[t[1] as usize];
                        let c = self.verts[t[2] as usize];
                        if let Some(w) = bary_2d([py, pz], [a[1], a[2]], [b[1], b[2]], [c[1], c[2]])
                        {
                            let xi = w[0] * a[0] + w[1] * b[0] + w[2] * c[0];
                            if xi > px + 1e-6 {
                                hits += 1;
                            }
                        }
                    }
                    g.set(x, y, z, if hits % 2 == 1 { 1 } else { -1 });
                }
            }
        }
        g
    }

    /// 面积加权表面采样：返回 `n` 个面元高斯（法向压薄 1/10）。
    pub fn surface_gaussians(&self, n: usize, degree: ShDegree) -> Vec<Gaussian> {
        let sh = target_sh(degree);
        self.surface_frames(n)
            .into_iter()
            .map(|(pos, normal, r)| Gaussian {
                pos,
                scale: [r, r, r * 0.1],
                rotation: quat_from_z(normal),
                opacity: 1.0,
                sh: sh.clone(),
            })
            .collect()
    }

    /// 面积加权采样：`(位置, 法线, 面元半径)`，确定性。
    pub fn surface_frames(&self, n: usize) -> Vec<([f32; 3], [f32; 3], f32)> {
        assert!(n > 0, "采样数必须 >0");
        let areas: Vec<f32> = self.tris.iter().map(|&t| self.tri_area(t)).collect();
        let total: f32 = areas.iter().sum();
        assert!(total > 0.0, "网格没有面积");
        let mut cdf = Vec::with_capacity(areas.len());
        let mut acc = 0.0f32;
        for a in &areas {
            acc += a;
            cdf.push(acc);
        }
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let u = (i as f32 + 0.5) / n as f32 * total;
            let ti = cdf.partition_point(|&v| v < u).min(self.tris.len() - 1);
            let t = self.tris[ti];
            let a = self.verts[t[0] as usize];
            let b = self.verts[t[1] as usize];
            let c = self.verts[t[2] as usize];
            let (gu, gv) = low_discrepancy(i);
            let w0 = 1.0 - gu - gv;
            let pos = [
                a[0] * w0 + b[0] * gu + c[0] * gv,
                a[1] * w0 + b[1] * gu + c[1] * gv,
                a[2] * w0 + b[2] * gu + c[2] * gv,
            ];
            let normal = normalize3(cross3(sub3(b, a), sub3(c, a)));
            let r = (total / n as f32 / PI).sqrt().max(0.005);
            out.push((pos, normal, r));
        }
        out
    }

    /// 面积加权表面采样：返回 `n` 个位置（确定性，用于 Chamfer 对照）。
    pub fn surface_points(&self, n: usize) -> Vec<[f32; 3]> {
        self.surface_frames(n)
            .into_iter()
            .map(|(p, _, _)| p)
            .collect()
    }
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-8 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// 把局部 +Z 轴旋到法线 `n` 的四元数 `[w, x, y, z]`。
fn quat_from_z(n: [f32; 3]) -> [f32; 4] {
    let z = normalize3(n);
    let d = z[2].clamp(-1.0, 1.0);
    if d < -0.999_999 {
        return [0.0, 1.0, 0.0, 0.0];
    }
    let axis = [-z[1], z[0], 0.0];
    let s = ((1.0 + d) * 2.0).sqrt();
    [s * 0.5, axis[0] / s, axis[1] / s, axis[2] / s]
}
/// 2D 重心坐标；点不在三角形内返回 `None`。
fn bary_2d(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> Option<[f32; 3]> {
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-9 {
        return None;
    }
    let w0 = ((b[1] - c[1]) * (p[0] - c[0]) + (c[0] - b[0]) * (p[1] - c[1])) / d;
    let w1 = ((c[1] - a[1]) * (p[0] - c[0]) + (a[0] - c[0]) * (p[1] - c[1])) / d;
    let w2 = 1.0 - w0 - w1;
    if w0 >= -1e-5 && w1 >= -1e-5 && w2 >= -1e-5 {
        Some([w0, w1, w2])
    } else {
        None
    }
}

/// 三角内确定性低差异点（`u+v ≤ 1`）。
fn low_discrepancy(j: usize) -> (f32, f32) {
    let g = 0.618_034;
    let u = (j as f32 + 0.5) * g;
    let u = u - u.floor();
    let v = (j as f32 + 0.5) * g * g;
    let v = v - v.floor();
    if u + v > 1.0 {
        (1.0 - u, 1.0 - v)
    } else {
        (u, v)
    }
}

/// 双向点云 Chamfer 平方距离。
pub fn chamfer_points(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let one = |from: &[[f32; 3]], to: &[[f32; 3]]| {
        from.iter()
            .map(|p| {
                to.iter()
                    .map(|q| {
                        let d = [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
                        d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
                    })
                    .fold(f32::INFINITY, f32::min)
            })
            .sum::<f32>()
            / from.len() as f32
    };
    one(a, b) * 0.5 + one(b, a) * 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torus_has_expected_counts() {
        let m = TriMesh::torus(0.7, 0.3, 12, 6);
        assert_eq!(m.verts.len(), 72);
        assert_eq!(m.tris.len(), 144);
        assert!(
            m.tris
                .iter()
                .all(|t| t.iter().all(|&i| (i as usize) < m.verts.len()))
        );
    }

    #[test]
    fn voxelize_torus_has_surface_and_hole() {
        let mut m = TriMesh::torus(0.7, 0.3, 24, 12);
        m.normalize();
        let g = m.voxelize(16);
        let solid = g.cells.iter().filter(|&&v| v > 0).count();
        assert!(solid > 100, "实体体素太少: {solid}");
        // 圆环中心（0,0,0）应在孔里（外部）。
        assert_eq!(g.get(8, 8, 8), -1, "圆环中心应为空");
    }

    #[test]
    fn surface_points_are_on_surface() {
        let mut m = TriMesh::torus(0.7, 0.3, 12, 6);
        m.normalize();
        let pts = m.surface_points(64);
        assert_eq!(pts.len(), 64);
        assert!(pts.iter().all(|p| p.iter().all(|v| v.abs() <= 1.0)));
        let gs = m.surface_gaussians(16, ShDegree::One);
        assert_eq!(gs.len(), 16);
        assert!(gs.iter().all(|g| g.sh.len() == 12));
    }
}
