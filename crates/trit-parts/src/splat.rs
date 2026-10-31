//! 高斯喷溅（f32）：多视角表面采样 → 各向同性高斯 → EWA 近似的 2D 投影 + 前到后 alpha 合成。
//!
//! 这一档**故意不走三值**：位置/尺度/颜色是连续量，三值化只会损失精度；
//! 三值留给「学」的部分。精度由采样格 `cell` 与高斯半径系数控制，形成精度阶梯。
//! 本轮不做梯度拟合（颜色直接取源像素）；拟合是下一步。

use std::collections::HashMap;

use crate::fuse::View;
use crate::geo::Camera;

/// 体素去重累加器：(位置和, 颜色和, 计数)。
type Acc = ([f64; 3], [f64; 3], u32);

/// 一个各向同性高斯。
#[derive(Clone, Debug)]
pub struct Splat {
    pub pos: [f32; 3],
    /// 世界系半径（米）。
    pub scale: f32,
    pub color: [f32; 3],
    pub alpha: f32,
}

/// 从多视角（id 图 + 深度 + RGB）建高斯：按世界体素去重，位置/颜色取均值。
/// `cell` 越大精度越低；`scale_k` 是半径相对体素的比例。
pub fn build(views: &[View], rgbs: &[Vec<f32>], cell: f32, scale_k: f32) -> Vec<Splat> {
    let mut acc: HashMap<(i32, i32, i32), Acc> = HashMap::new();
    let inv = 1.0 / cell.max(1e-5);
    for (v, rgb) in views.iter().zip(rgbs) {
        if rgb.len() != v.w * v.h * 3 {
            continue;
        }
        for i in 0..v.ids.len().min(v.w * v.h) {
            let d = v.depth[i];
            if v.ids[i] == 0 || !(d.is_finite() && d > 0.0 && d < 50.0) {
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

/// 渲染一帧：近到远 alpha 合成（EWA 的简化：屏幕空间各向同性高斯）。
pub fn render(splats: &[Splat], cam: &Camera, w: usize, h: usize, bg: [f32; 3]) -> Vec<f32> {
    let fx = cam.k[0][0];
    struct Item {
        depth: f64,
        u: f64,
        v: f64,
        sigma: f64,
        color: [f32; 3],
        alpha: f32,
    }
    let mut items: Vec<Item> = Vec::with_capacity(splats.len());
    for s in splats {
        let Some((u, v, d)) = cam.project(s.pos) else {
            continue;
        };
        if u < -2.0 || v < -2.0 || u >= w as f64 + 2.0 || v >= h as f64 + 2.0 {
            continue;
        }
        // 世界半径 → 像素：sigma ≈ f * r / z
        let sigma = (fx * s.scale as f64 / d).max(0.35);
        items.push(Item {
            depth: d,
            u,
            v,
            sigma,
            color: s.color,
            alpha: s.alpha,
        });
    }
    items.sort_by(|a, b| a.depth.total_cmp(&b.depth));
    let mut acc = vec![0.0f32; w * h * 3];
    let mut t = vec![0.0f32; w * h]; // 透射率累积的“已覆盖”量
    for it in &items {
        let r = (it.sigma * 2.5).ceil() as i64;
        let cx = it.u.round() as i64;
        let cy = it.v.round() as i64;
        for y in (cy - r).max(0)..=(cy + r).min(h as i64 - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(w as i64 - 1) {
                let dx = (x as f64 - it.u) / it.sigma;
                let dy = (y as f64 - it.v) / it.sigma;
                let g = (-0.5 * (dx * dx + dy * dy)).exp();
                if g < 1e-3 {
                    continue;
                }
                let p = y as usize * w + x as usize;
                let a = (it.alpha as f64 * g) as f32;
                let trans = 1.0 - t[p];
                let wgt = a * trans;
                for c in 0..3 {
                    acc[p * 3 + c] += wgt * it.color[c];
                }
                t[p] += wgt;
            }
        }
    }
    for p in 0..w * h {
        let rest = (1.0 - t[p]).max(0.0);
        for c in 0..3 {
            acc[p * 3 + c] += rest * bg[c];
        }
    }
    acc
}

/// PSNR（输入 0..1）。
pub fn psnr(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len(), "PSNR 输入长度不一致");
    let mse: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x - *y);
            d * d
        })
        .sum::<f64>()
        / a.len().max(1) as f64;
    if mse <= 1e-12 {
        return f64::INFINITY;
    }
    10.0 * (1.0 / mse).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam() -> Camera {
        Camera {
            k: [[100.0, 0.0, 50.0], [0.0, 100.0, 50.0], [0.0, 0.0, 1.0]],
            t_cw: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    #[test]
    fn single_splat_renders_at_projection() {
        let c = cam();
        // 世界 (0,0,-2) 投到像素 (50,50)
        let s = Splat {
            pos: [0.0, 0.0, -2.0],
            scale: 0.05,
            color: [1.0, 0.0, 0.0],
            alpha: 1.0,
        };
        let img = render(&[s], &c, 100, 100, [0.0, 0.0, 0.0]);
        let p = 50 * 100 + 50;
        assert!(img[p * 3] > 0.5, "中心应为红：{}", img[p * 3]);
        assert!(img[p * 3 + 1] < 0.1);
    }

    #[test]
    fn near_splat_occludes_far_one() {
        let c = cam();
        let near = Splat {
            pos: [0.0, 0.0, -1.0],
            scale: 0.05,
            color: [0.0, 1.0, 0.0],
            alpha: 1.0,
        };
        let far = Splat {
            pos: [0.0, 0.0, -3.0],
            scale: 0.05,
            color: [0.0, 0.0, 1.0],
            alpha: 1.0,
        };
        let img = render(&[far, near], &c, 100, 100, [0.0, 0.0, 0.0]);
        let p = 50 * 100 + 50;
        assert!(img[p * 3 + 1] > img[p * 3 + 2], "近处绿色应盖住远处蓝色");
    }

    #[test]
    fn psnr_identical_is_infinite() {
        let a = vec![0.5f32; 12];
        assert!(psnr(&a, &a).is_infinite());
        let b = vec![0.0f32; 12];
        let v = psnr(&a, &b);
        assert!(
            (v - 6.0206).abs() < 1e-3,
            "0.5 vs 0 的 PSNR 应为 6.02 dB，得到 {v}"
        );
    }

    #[test]
    fn build_dedups_by_voxel() {
        // 5×5 全前景、深度 2、颜色 (1,0,0)：cell=0.5 m 时点被并成少数几个高斯
        let (w, h) = (5usize, 5usize);
        let view = View {
            ids: vec![1u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam(),
        };
        let rgb = vec![1.0f32; w * h * 3];
        let fine = build(std::slice::from_ref(&view), std::slice::from_ref(&rgb), 0.01, 2.0);
        assert!(fine.len() > 10, "细格子应有很多高斯：{}", fine.len());
        let view2 = View {
            ids: vec![1u32; w * h],
            depth: vec![2.0; w * h],
            w,
            h,
            cam: cam(),
        };
        let coarse = build(&[view2], &[rgb], 0.5, 2.0);
        assert!(
            coarse.len() < fine.len(),
            "粗格子应更少：{} vs {}",
            coarse.len(),
            fine.len()
        );
        assert!((coarse[0].color[0] - 1.0).abs() < 1e-6);
    }
}
