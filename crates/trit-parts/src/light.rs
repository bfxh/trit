//! 单图光照估计与**平面近似**重打光（f32，纯 std，确定性）。
//!
//! 老实说清单图逆渲染的本质歧义：单张图的**仰角**与反照率/几何纠缠，无法可靠分离。
//! 这里只承诺三件可测的事：
//! 1. **方位角**（光从哪个方向来）——用低频亮度的梯度方向估计，符号由「亮度朝哪边增加」定；
//! 2. **掠射程度**（低角度光看起来更「斜」）——归一化梯度能量；
//! 3. **色温**（亮部 vs 暗部的 R−B 差）。
//!
//! 重打光用**线性平面**近似光照（一阶），旋转梯度后按 `albedo = I / S` 重组；
//! 对强方向光/大尺度渐变够用，对复杂多点光不成立。

use crate::tex::box_blur_rgb;

pub struct LightEst {
    /// 方位角（度，0..360；0 = 光来自 +x，逆时针）。实现约定：光来自「亮度下降」的一侧。
    pub azimuth_deg: f32,
    /// 掠射程度：低频梯度能量 / 低频亮度范围（越大越斜）。
    pub grazing: f32,
    /// 平均亮度（0..1）。
    pub mean_luma: f32,
    /// 亮部−暗部的 (R−B)：>0 偏暖。
    pub warmth: f32,
}

fn luma(p: &[f32]) -> f32 {
    0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]
}

/// 估计光照（低频亮度 = shading 的替身，半径取短边的 1/16）。
pub fn estimate(img: &[f32], w: usize, h: usize) -> LightEst {
    let r = (w.min(h) / 16).max(2);
    let s = box_blur_rgb(img, w, h, r);
    let mut lum = vec![0.0f32; w * h];
    for i in 0..w * h {
        lum[i] = luma(&s[i * 3..i * 3 + 3]);
    }
    // 梯度（中心差分）
    let (mut gx, mut gy) = (0.0f64, 0.0f64);
    let (mut mag, mut n) = (0.0f64, 0usize);
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let dx = (lum[y * w + x + 1] - lum[y * w + x - 1]) as f64;
            let dy = (lum[(y + 1) * w + x] - lum[(y - 1) * w + x]) as f64;
            gx += dx;
            gy += dy;
            mag += (dx * dx + dy * dy).sqrt();
            n += 1;
        }
    }
    // 亮度**增加**的方向 = 远离光 ⇒ 光来自相反方向
    // 角度约定：0 = +x，逆时针为正 ⇒ atan2(gy, gx)
    let light_dir = gy.atan2(gx) + std::f64::consts::PI;
    let mut az = light_dir.to_degrees() as f32 % 360.0;
    if az < 0.0 {
        az += 360.0;
    }
    // 亮度范围（p5..p95）做归一化
    let mut ls: Vec<f32> = lum.clone();
    ls.sort_by(f32::total_cmp);
    let q = |f: f32| ls[((ls.len() as f32 - 1.0) * f).round() as usize];
    let range = (q(0.95) - q(0.05)).max(1e-4);
    let grazing = (mag / n.max(1) as f64 / range as f64) as f32;
    // 色温：亮部 10% 与暗部 10% 的 (R−B) 差
    let mean_luma = ls.iter().sum::<f32>() / ls.len().max(1) as f32;
    let hi_lo = q(0.9);
    let lo_lo = q(0.1);
    let mut warm_hi = 0.0f32;
    let mut warm_lo = 0.0f32;
    let (mut nh, mut nl) = (0usize, 0usize);
    for i in 0..w * h {
        let l = lum[i];
        let d = s[i * 3] - s[i * 3 + 2];
        if l >= hi_lo {
            warm_hi += d;
            nh += 1;
        } else if l <= lo_lo {
            warm_lo += d;
            nl += 1;
        }
    }
    let warmth = warm_hi / nh.max(1) as f32 - warm_lo / nl.max(1) as f32;
    LightEst {
        azimuth_deg: az,
        grazing,
        mean_luma,
        warmth,
    }
}

/// 线性平面拟合 `a + bx·u + by·v`（u,v ∈ `[0,1]`）。
fn fit_plane(v: &[f32], w: usize, h: usize) -> [f32; 3] {
    let n = (w * h) as f64;
    let (mut su, mut sv, mut suu, mut svv, mut suv, mut sy, mut suy, mut svy) = (
        0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64,
    );
    for y in 0..h {
        let vv = y as f64 / h.max(1) as f64;
        for x in 0..w {
            let u = x as f64 / w.max(1) as f64;
            su += u;
            sv += vv;
            suu += u * u;
            svv += vv * vv;
            suv += u * vv;
            let val = v[y * w + x] as f64;
            sy += val;
            suy += val * u;
            svy += val * vv;
        }
    }
    // 3×3 求解（Cramer）
    let m = [[n, su, sv], [su, suu, suv], [sv, suv, svv]];
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return [sy / n, 0.0, 0.0].map(|x| x as f32);
    }
    let b = [sy, suy, svy];
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        let mut mm = m;
        for r in 0..3 {
            mm[r][k] = b[r];
        }
        let d = mm[0][0] * (mm[1][1] * mm[2][2] - mm[1][2] * mm[2][1])
            - mm[0][1] * (mm[1][0] * mm[2][2] - mm[1][2] * mm[2][0])
            + mm[0][2] * (mm[1][0] * mm[2][1] - mm[1][1] * mm[2][0]);
        out[k] = (d / det) as f32;
    }
    out
}

/// 重打光：`albedo = I / S`，把平面光照的梯度旋转 `dtheta_deg` 后重组。
///
/// 约定与 [`estimate`] 一致：平面 `S = a + bx·u + by·v`，其中梯度方向指向「亮度增加」；
/// 旋转 `+θ` 表示光方位角增加 `θ`（逆时针）。
pub fn relight(img: &[f32], w: usize, h: usize, dtheta_deg: f32) -> Vec<f32> {
    let r = (w.min(h) / 16).max(2);
    let s = box_blur_rgb(img, w, h, r);
    let mut lum = vec![0.0f32; w * h];
    for i in 0..w * h {
        lum[i] = luma(&s[i * 3..i * 3 + 3]);
    }
    let p = fit_plane(&lum, w, h);
    let (bx, by) = (p[1], p[2]);
    let th = dtheta_deg.to_radians();
    let (bx2, by2) = (bx * th.cos() - by * th.sin(), bx * th.sin() + by * th.cos());
    let mut out = vec![0.0f32; img.len()];
    for y in 0..h {
        let v = y as f32 / h.max(1) as f32;
        for x in 0..w {
            let u = x as f32 / w.max(1) as f32;
            let s_old = (p[0] + bx * u + by * v).clamp(0.05, 1.0);
            let s_new = (p[0] + bx2 * u + by2 * v).clamp(0.05, 1.0);
            let i = (y * w + x) * 3;
            for c in 0..3 {
                let albedo = img[i + c] / s_old;
                out[i + c] = (albedo * s_new).clamp(0.0, 1.0);
            }
        }
    }
    out
}

/// 由「法线 + 亮度」线性求解光照：`I ≈ a + b·n`，`b = k·l`。
/// 返回 `(l 单位向量, k 强度, a 环境项)`；y 方向沿用图像坐标（v 向下）。
///
/// 前提：法线图与亮度同一相机坐标系；只用 `mask` 为真的像素。
pub fn fit_from_normals(
    lum: &[f32],
    n: &[f32],
    mask: &[bool],
    w: usize,
    h: usize,
) -> Option<([f32; 3], f32, f32)> {
    // 法方程 4×4：未知 [a, bx, by, bz]
    let mut ata = [[0.0f64; 4]; 4];
    let mut aty = [0.0f64; 4];
    let mut cnt = 0usize;
    for i in 0..w * h {
        if !mask[i] {
            continue;
        }
        let row = [
            1.0,
            n[i * 3] as f64,
            n[i * 3 + 1] as f64,
            n[i * 3 + 2] as f64,
        ];
        let y = lum[i] as f64;
        for a in 0..4 {
            aty[a] += row[a] * y;
            for b in 0..4 {
                ata[a][b] += row[a] * row[b];
            }
        }
        cnt += 1;
    }
    if cnt < 64 {
        return None;
    }
    let mut m = ata;
    let mut b = aty;
    let n4 = 4;
    for col in 0..n4 {
        let mut piv = col;
        for r in col + 1..n4 {
            if m[r][col].abs() > m[piv][col].abs() {
                piv = r;
            }
        }
        if m[piv][col].abs() < 1e-12 {
            return None;
        }
        m.swap(col, piv);
        b.swap(col, piv);
        let d = m[col][col];
        for v in m[col].iter_mut().skip(col) {
            *v /= d;
        }
        b[col] /= d;
        for r in 0..n4 {
            if r == col {
                continue;
            }
            let f = m[r][col];
            let src: [f64; 4] = m[col];
            for (v, s0) in m[r].iter_mut().skip(col).zip(src.iter().skip(col)) {
                *v -= f * s0;
            }
            b[r] -= f * b[col];
        }
    }
    let (a, bx, by, bz) = (b[0], b[1], b[2], b[3]);
    let k = (bx * bx + by * by + bz * bz).sqrt();
    if k < 1e-9 {
        return None;
    }
    Some((
        [(bx / k) as f32, (by / k) as f32, (bz / k) as f32],
        k as f32,
        a as f32,
    ))
}

/// 基于法线的重打光：`L' = a + k·max(0, n·l')`，按 `I' = I · L'/L` 缩放（保色相）。
pub fn relight_normals(
    rgb: &[f32],
    n: &[f32],
    w: usize,
    h: usize,
    l: [f32; 3],
    k: f32,
    a: f32,
) -> Vec<f32> {
    let mut out = rgb.to_vec();
    for i in 0..w * h {
        let ndl = n[i * 3] * l[0] + n[i * 3 + 1] * l[1] + n[i * 3 + 2] * l[2];
        let l_new = (a + k * ndl.max(0.0)).clamp(0.02, 2.0);
        let l_old =
            (0.299 * rgb[i * 3] + 0.587 * rgb[i * 3 + 1] + 0.114 * rgb[i * 3 + 2]).clamp(0.02, 1.0);
        let s = (l_new / l_old).clamp(0.1, 6.0);
        for c in 0..3 {
            out[i * 3 + c] = (rgb[i * 3 + c] * s).clamp(0.0, 1.0);
        }
    }
    out
}

/// 把光方向绕 z 轴旋转 `deg`（世界系，逆时针）。
pub fn rotate_light(l: [f32; 3], deg: f32) -> [f32; 3] {
    let t = deg.to_radians();
    [
        l[0] * t.cos() - l[1] * t.sin(),
        l[0] * t.sin() + l[1] * t.cos(),
        l[2],
    ]
}

/// 两单位向量的夹角（度）。
pub fn angle_between(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = (a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).clamp(-1.0, 1.0);
    d.acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 亮度沿 x 增加（光来自左侧 ⇒ 方位角 ≈ 180°）
    fn ramp_image(w: usize, h: usize, dir: [f32; 2]) -> Vec<f32> {
        let mut v = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let u = x as f32 / w as f32;
                let vv = y as f32 / h as f32;
                let c = (0.3 + 0.4 * (u * dir[0] + vv * dir[1])).clamp(0.0, 1.0);
                let i = (y * w + x) * 3;
                v[i] = c;
                v[i + 1] = c;
                v[i + 2] = c;
            }
        }
        v
    }

    #[test]
    fn azimuth_points_to_the_dark_side() {
        // 沿 +x 变亮 ⇒ 光来自 −x ⇒ 方位角 ≈ 180°
        let img = ramp_image(64, 64, [1.0, 0.0]);
        let e = estimate(&img, 64, 64);
        assert!(
            (e.azimuth_deg - 180.0).abs() < 12.0,
            "方位角应 ≈180°，实际 {:.1}",
            e.azimuth_deg
        );
        // 沿 +y 变亮 ⇒ 光来自 −y ⇒ 方位角 ≈ 270°（约定 0=+x，逆时针）
        let img = ramp_image(64, 64, [0.0, 1.0]);
        let e = estimate(&img, 64, 64);
        assert!(
            (e.azimuth_deg - 270.0).abs() < 12.0,
            "方位角应 ≈270°，实际 {:.1}",
            e.azimuth_deg
        );
    }

    #[test]
    fn relight_rotates_the_estimated_azimuth() {
        let (w, h) = (64usize, 64usize);
        let img = ramp_image(w, h, [1.0, 0.0]);
        let a0 = estimate(&img, w, h).azimuth_deg;
        let out = relight(&img, w, h, 90.0);
        let a1 = estimate(&out, w, h).azimuth_deg;
        let d = ((a1 - a0 + 540.0) % 360.0) - 180.0;
        assert!(
            (d.abs() - 90.0).abs() < 20.0,
            "重打光 90° 后方位角变化应 ≈±90°，实际 {d:.1}（{a0:.1}→{a1:.1}）"
        );
    }

    #[test]
    fn normals_fit_recovers_a_known_light() {
        // 半球法线 + 已知光 ⇒ 线性拟合应几乎精确恢复 l 与 k
        let (w, h) = (32usize, 32usize);
        let l = {
            let v = [0.4f32, -0.5, 0.75];
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            [v[0] / n, v[1] / n, v[2] / n]
        };
        let (k, a) = (0.8f32, 0.15f32);
        let mut nrm = vec![0.0f32; w * h * 3];
        let mut lum = vec![0.0f32; w * h];
        let mask = vec![true; w * h];
        for y in 0..h {
            for x in 0..w {
                let u = (x as f32 + 0.5) / w as f32 * 2.0 - 1.0;
                let v = (y as f32 + 0.5) / h as f32 * 2.0 - 1.0;
                let nz = (1.0f32 - u * u - v * v).max(0.0).sqrt();
                let i = y * w + x;
                nrm[i * 3] = u;
                nrm[i * 3 + 1] = v;
                nrm[i * 3 + 2] = nz;
                lum[i] = a + k * (u * l[0] + v * l[1] + nz * l[2]);
            }
        }
        let (lhat, khat, ahat) = fit_from_normals(&lum, &nrm, &mask, w, h).unwrap();
        assert!(
            angle_between(l, lhat) < 3.0,
            "光方向应几乎一致，误差 {:.2}°",
            angle_between(l, lhat)
        );
        assert!((khat - k).abs() < 0.02, "强度 {khat} vs {k}");
        assert!((ahat - a).abs() < 0.02, "环境项 {ahat} vs {a}");
    }

    #[test]
    fn is_deterministic() {
        let img = ramp_image(32, 32, [0.7, 0.7]);
        let a = estimate(&img, 32, 32);
        let b = estimate(&img, 32, 32);
        assert_eq!(a.azimuth_deg, b.azimuth_deg);
        assert_eq!(relight(&img, 32, 32, 45.0), relight(&img, 32, 32, 45.0));
    }
}
