//! 几何侧：相机反投影、体素化、3D IoU、点云 PLY 导出。
//!
//! 这一层只吃「id 图 + 深度 + 相机」三样东西，不关心 id 是预测的还是 GT 的，
//! 所以同一套代码既能给模型评分，也能直接导出 `.ply` 交给下游（体素化/提网格）。

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// 相机：`K` 内参；`t_cw` 存的是**相机→世界**（渲染 meta 里叫 `T_wc`）。
/// ⚠️ meta 的 `T_cw` 是它的逆（世界→相机）；早先版本误把 `T_cw` 当正变换，
/// 单帧自洽看不出来，跨视角融合时才会暴露（2026-10-31 N1 实测 IoU≈0）。
#[derive(Clone, Debug)]
pub struct Camera {
    pub k: [[f64; 3]; 3],
    pub t_cw: [[f64; 4]; 4],
}

impl Camera {
    pub fn load(path: &Path) -> Result<Self, String> {
        let s = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let k = matrix(&s, "K")?;
        let t = if s.contains("\"T_wc\"") {
            matrix(&s, "T_wc")?
        } else {
            matrix(&s, "T_cw")?
        };
        if k.len() != 3 || k.iter().any(|r| r.len() != 3) {
            return Err("K 不是 3×3".into());
        }
        if t.len() != 4 || t.iter().any(|r| r.len() != 4) {
            return Err("T_cw 不是 4×4".into());
        }
        let mut cam = Camera {
            k: [[0.0; 3]; 3],
            t_cw: [[0.0; 4]; 4],
        };
        for (dst, src) in cam.k.iter_mut().zip(k.iter()) {
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d = *s;
            }
        }
        for (dst, src) in cam.t_cw.iter_mut().zip(t.iter()) {
            for (d, s) in dst.iter_mut().zip(src.iter()) {
                *d = *s;
            }
        }
        Ok(cam)
    }

    /// 像素 + 深度（沿视线，米）→ 世界坐标。
    ///
    /// 约定：Blender 相机**看 −Z、+Y 朝上**，像素 y 向下 ⇒ 相机系向量是
    /// `(x, −y, −d)`（OpenCV 的 y/z 与 Blender 反号）。不翻这两个号，点会落到
    /// 相机背后——单帧自洽发现不了，多视角融合时会散开（2026-10-31 实测）。
    pub fn unproject(&self, u: f64, v: f64, d: f32) -> [f32; 3] {
        let (fx, fy) = (self.k[0][0], self.k[1][1]);
        let (cx, cy) = (self.k[0][2], self.k[1][2]);
        let z = f64::from(d);
        let xc = (u - cx) * z / fx;
        let yc = -(v - cy) * z / fy;
        let zc = -z;
        let t = &self.t_cw;
        [
            (t[0][0] * xc + t[0][1] * yc + t[0][2] * zc + t[0][3]) as f32,
            (t[1][0] * xc + t[1][1] * yc + t[1][2] * zc + t[1][3]) as f32,
            (t[2][0] * xc + t[2][1] * yc + t[2][2] * zc + t[2][3]) as f32,
        ]
    }
}

impl Camera {
    /// 世界点 → 像素 + 深度（沿视线，米）；在相机背后返回 `None`。
    pub fn project(&self, p: [f32; 3]) -> Option<(f64, f64, f64)> {
        let t = &self.t_cw;
        let (dx, dy, dz) = (
            p[0] as f64 - t[0][3],
            p[1] as f64 - t[1][3],
            p[2] as f64 - t[2][3],
        );
        // R^T (p − t)：t_cw 的旋转部分是 world→camera 的 R
        let xb = t[0][0] * dx + t[1][0] * dy + t[2][0] * dz;
        let yb = t[0][1] * dx + t[1][1] * dy + t[2][1] * dz;
        let zb = t[0][2] * dx + t[1][2] * dy + t[2][2] * dz;
        if zb >= 0.0 {
            return None; // Blender 相机看 −Z；zb ≥ 0 表示在背后
        }
        let d = -zb;
        let x = xb;
        let y = -yb;
        let (fx, fy) = (self.k[0][0], self.k[1][1]);
        let (cx, cy) = (self.k[0][2], self.k[1][2]);
        Some((fx * x / d + cx, fy * y / d + cy, d))
    }
}

/// 读 `.npy`（仅小端 f32，2 维），返回 `(数据, 形状)`。
pub fn read_npy_f32(path: &Path) -> Result<(Vec<f32>, Vec<usize>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() < 10 || &bytes[0..6] != b"\x93NUMPY" {
        return Err("不是 .npy 文件".into());
    }
    let major = bytes[6];
    let (hlen, start) = if major == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        (
            u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize,
            12,
        )
    };
    let header = std::str::from_utf8(&bytes[start..start + hlen]).map_err(|_| "npy 头非 UTF-8")?;
    let descr = npy_field(header, "descr").ok_or("npy 缺 descr")?;
    if descr != "<f4" && descr != "|f4" {
        return Err(format!("只支持 float32，得到 {descr}"));
    }
    let shape: Vec<usize> = npy_field(header, "shape")
        .ok_or("npy 缺 shape")?
        .trim_matches(|c| c == '(' || c == ')' || c == ',' || c == ' ')
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let n: usize = shape.iter().product();
    let body = &bytes[start + hlen..];
    if body.len() < n * 4 {
        return Err("npy 数据长度不足".into());
    }
    let data = body[..n * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect();
    Ok((data, shape))
}

fn npy_field<'a>(header: &'a str, key: &str) -> Option<&'a str> {
    let i = header.find(&format!("'{key}'"))? + key.len() + 2;
    let rest = header[i..].trim_start().strip_prefix(':')?.trim_start();
    if let Some(r) = rest.strip_prefix('\'') {
        let b = r.find('\'')?;
        Some(&r[..b])
    } else {
        let end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '(' || c == ')' || c == ',' || c == ' '))
            .unwrap_or(rest.len());
        Some(rest[..end].trim_end())
    }
}

/// 从 JSON 里取一个二维数字矩阵（`[[..],[..]]`）。
fn matrix(json: &str, key: &str) -> Result<Vec<Vec<f64>>, String> {
    let i = json
        .find(&format!("\"{key}\""))
        .ok_or_else(|| format!("缺 {key}"))?;
    let s = &json[i..];
    let start = s.find('[').ok_or("缺 [")?;
    let mut depth = 0i32;
    let mut rows: Vec<Vec<f64>> = Vec::new();
    let mut row: Vec<f64> = Vec::new();
    let mut tok = String::new();
    let flush = |tok: &mut String, row: &mut Vec<f64>| {
        if let Ok(v) = tok.trim().parse::<f64>() {
            row.push(v);
        }
        tok.clear();
    };
    for c in s[start..].chars() {
        match c {
            '[' => depth += 1,
            ']' => {
                flush(&mut tok, &mut row);
                if depth == 2 && !row.is_empty() {
                    rows.push(std::mem::take(&mut row));
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            ',' => flush(&mut tok, &mut row),
            _ => tok.push(c),
        }
    }
    Ok(rows)
}

/// 逐像素反投影，按部件 id 分组（深度非法处跳过）。
pub fn clouds(
    ids: &[u32],
    depth: &[f32],
    cam: &Camera,
    w: usize,
    h: usize,
) -> BTreeMap<u32, Vec<[f32; 3]>> {
    let mut out: BTreeMap<u32, Vec<[f32; 3]>> = BTreeMap::new();
    for v in 0..h {
        for u in 0..w {
            let idx = v * w + u;
            let id = ids[idx];
            let d = depth[idx];
            if id == 0 || !(d.is_finite() && d > 0.0 && d < 50.0) {
                continue;
            }
            out.entry(id)
                .or_default()
                .push(cam.unproject(u as f64, v as f64, d));
        }
    }
    out
}

pub type Voxel = (i32, i32, i32);

/// 点云 → 占据体素集合（边长 `voxel` 米）。
pub fn voxelize(points: &[[f32; 3]], voxel: f32) -> HashSet<Voxel> {
    let inv = 1.0 / voxel;
    points
        .iter()
        .map(|p| {
            [
                (p[0] * inv).floor() as i32,
                (p[1] * inv).floor() as i32,
                (p[2] * inv).floor() as i32,
            ]
        })
        .map(|v| (v[0], v[1], v[2]))
        .collect()
}

pub fn iou3d(a: &HashSet<Voxel>, b: &HashSet<Voxel>) -> f32 {
    if a.is_empty() && b.is_empty() {
        return f32::NAN;
    }
    let inter = a.intersection(b).count() as f32;
    let union = (a.len() + b.len()) as f32 - inter;
    if union == 0.0 {
        return f32::NAN;
    }
    inter / union
}

/// 质心（米）。
pub fn centroid(points: &[[f32; 3]]) -> [f32; 3] {
    if points.is_empty() {
        return [f32::NAN; 3];
    }
    let mut s = [0.0f64; 3];
    for p in points {
        s[0] += f64::from(p[0]);
        s[1] += f64::from(p[1]);
        s[2] += f64::from(p[2]);
    }
    let n = points.len() as f64;
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

/// 写出 PLY（ASCII，逐部件着色：id → 固定色）。
pub fn write_ply(path: &Path, parts: &BTreeMap<u32, Vec<[f32; 3]>>) -> std::io::Result<()> {
    use std::fmt::Write as _;
    let total: usize = parts.values().map(Vec::len).sum();
    let mut s = String::with_capacity(total * 40 + 256);
    writeln!(s, "ply").ok();
    writeln!(s, "format ascii 1.0").ok();
    writeln!(s, "element vertex {total}").ok();
    writeln!(s, "property float x").ok();
    writeln!(s, "property float y").ok();
    writeln!(s, "property float z").ok();
    writeln!(s, "property uchar red").ok();
    writeln!(s, "property uchar green").ok();
    writeln!(s, "property uchar blue").ok();
    writeln!(s, "end_header").ok();
    for (&id, pts) in parts {
        let (r, g, b) = part_color(id);
        for p in pts {
            writeln!(s, "{:.4} {:.4} {:.4} {r} {g} {b}", p[0], p[1], p[2]).ok();
        }
    }
    std::fs::write(path, s)
}

/// 部件 id → 稳定可辨的颜色。
pub fn part_color(id: u32) -> (u8, u8, u8) {
    let h = (id.wrapping_mul(2654435761) >> 8) & 0xffff;
    let r = (h & 0xff) as u8;
    let g = ((h >> 8) & 0xff) as u8;
    let b = (((h >> 4) ^ (id & 0x33)) & 0xff) as u8;
    (r.max(40), g.max(40), b.max(40))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unproject_identity_camera() {
        let cam = Camera {
            k: [[100.0, 0.0, 0.0], [0.0, 100.0, 0.0], [0.0, 0.0, 1.0]],
            t_cw: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        // Blender 约定：相机看 −Z、像素 y 向下 ⇒ 相机系向量 (x, −y, −d)
        let p = cam.unproject(10.0, 20.0, 2.0);
        assert!((p[0] - 0.2).abs() < 1e-6, "{p:?}");
        assert!((p[1] + 0.4).abs() < 1e-6, "{p:?}");
        assert!((p[2] + 2.0).abs() < 1e-6, "{p:?}");
        // project 与 unproject 必须互为逆（否则多视角融合会散）
        let (u, v, d) = cam.project(p).expect("在相机前方");
        assert!((u - 10.0).abs() < 1e-6 && (v - 20.0).abs() < 1e-6 && (d - 2.0).abs() < 1e-6, "{u} {v} {d}");
    }

    #[test]
    fn iou_is_one_for_identical_sets() {
        let pts = vec![[0.0, 0.0, 0.0], [0.001, 0.0, 0.0], [0.0, 0.05, 0.0]];
        let a = voxelize(&pts, 0.01);
        let b = voxelize(&pts, 0.01);
        assert_eq!(a.len(), 2);
        assert!((iou3d(&a, &b) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn iou_is_zero_for_disjoint_sets() {
        let a = voxelize(&[[0.0, 0.0, 0.0]], 0.01);
        let b = voxelize(&[[1.0, 0.0, 0.0]], 0.01);
        assert_eq!(iou3d(&a, &b), 0.0);
    }
}
