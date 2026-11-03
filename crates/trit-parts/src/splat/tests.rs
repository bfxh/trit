use crate::fuse::View;
use crate::geo::Camera;

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

fn loss_of(splats: &[Splat], views: &[FitView]) -> f64 {
    let mut l = 0.0;
    for (cam, w, h, gt, ids) in views {
        let c = build_cache(splats, cam, *w, *h);
        let img = render_cached(splats, &c, *w, *h, [0.0, 0.0, 0.0]);
        for p in 0..*w * *h {
            if ids[p] == 0 {
                continue;
            }
            for k in 0..3 {
                let d = f64::from(img[p * 3 + k] - gt[p * 3 + k]);
                l += d * d;
            }
        }
    }
    l
}

#[test]
fn jacobian_of_projection_matches_numeric() {
    let c = cam();
    let s = Splat {
        pos: [0.3, -0.2, -2.0],
        scale: 0.05,
        color: [1.0; 3],
        alpha: 0.8,
    };
    let base = c.project(s.pos).unwrap();
    let eps = 1e-3; // f32 位置：差分步要够大
    for a in 0..3 {
        let mut q = s.pos;
        q[a] += eps as f32;
        let up = c.project(q).unwrap();
        for (name, num, ana) in [
            ("u", (up.0 - base.0) / eps, {
                // 解析：fx·(R[a][0]·D + xb·R[a][2])/D²
                let t = &c.t_cw;
                let (xb, _yb) = (
                    t[0][0] * q[0] as f64 + t[1][0] * q[1] as f64 + t[2][0] * q[2] as f64,
                    t[0][1] * q[0] as f64 + t[1][1] * q[1] as f64 + t[2][1] * q[2] as f64,
                );
                let d = base.2;
                c.k[0][0] * (t[a][0] * d + xb * t[a][2]) / (d * d)
            }),
            ("v", (up.1 - base.1) / eps, {
                let t = &c.t_cw;
                let (_xb, yb) = (
                    t[0][0] * q[0] as f64 + t[1][0] * q[1] as f64 + t[2][0] * q[2] as f64,
                    t[0][1] * q[0] as f64 + t[1][1] * q[1] as f64 + t[2][1] * q[2] as f64,
                );
                let d = base.2;
                c.k[1][1] * (-t[a][1] * d + (-yb) * t[a][2]) / (d * d)
            }),
            ("d", (up.2 - base.2) / eps, -c.t_cw[a][2]),
        ] {
            assert!(
                (num - ana).abs() < 5e-3 * ana.abs().max(1.0),
                "axis {a} {name}: 数值 {num} vs 解析 {ana}"
            );
        }
    }
    let _ = s;
}

#[test]
fn analytic_geometry_gradients_match_finite_difference() {
    let c = cam();
    let (w, h) = (100usize, 100usize);
    let s0 = Splat {
        pos: [0.0, 0.0, -2.0],
        scale: 0.05,
        color: [1.0, 0.2, 0.2],
        alpha: 0.8,
    };
    let gt = [0.1f32, 0.9, 0.2].repeat(w * h);
    let ids = vec![1u32; w * h];
    let views = vec![(c.clone(), w, h, gt, ids)];
    let (_gc, gp, gs, ga, cnt) = gradients(std::slice::from_ref(&s0), &views, [0.0, 0.0, 0.0]);
    assert!(cnt[0] > 0, "高斯应覆盖到像素");
    let _ = cnt;
    let eps = 1e-4;
    // 位置 x
    for (a, &ana) in gp[0].iter().enumerate() {
        let mut up = s0.clone();
        up.pos[a] += eps as f32;
        let mut dn = s0.clone();
        dn.pos[a] -= eps as f32;
        let num = (loss_of(&[up], &views) - loss_of(&[dn], &views)) / (2.0 * eps);
        assert!(
            (num - ana).abs() < 5e-2 * ana.abs().max(1e-3) + 1e-3,
            "pos[{a}]: 数值 {num:.6} vs 解析 {ana:.6}"
        );
    }
    // 尺度
    let mut up = s0.clone();
    up.scale += eps as f32;
    let mut dn = s0.clone();
    dn.scale -= eps as f32;
    let num = (loss_of(&[up], &views) - loss_of(&[dn], &views)) / (2.0 * eps);
    let ana = gs[0];
    assert!(
        (num - ana).abs() < 5e-2 * ana.abs().max(1e-3) + 1e-3,
        "scale: 数值 {num:.6} vs 解析 {ana:.6}"
    );
    // alpha
    let mut up = s0.clone();
    up.alpha += eps as f32;
    let mut dn = s0.clone();
    dn.alpha -= eps as f32;
    let num = (loss_of(&[up], &views) - loss_of(&[dn], &views)) / (2.0 * eps);
    let ana = ga[0];
    assert!(
        (num - ana).abs() < 5e-2 * ana.abs().max(1e-3) + 1e-3,
        "alpha: 数值 {num:.6} vs 解析 {ana:.6}"
    );
}

#[test]
fn two_splat_transmittance_chain_matches_numeric() {
    let c = cam();
    let (w, h) = (100usize, 100usize);
    let near = Splat {
        pos: [0.0, 0.0, -1.6],
        scale: 0.06,
        color: [0.9, 0.1, 0.1],
        alpha: 0.7,
    };
    let far = Splat {
        pos: [0.05, 0.0, -2.2],
        scale: 0.06,
        color: [0.1, 0.1, 0.9],
        alpha: 0.7,
    };
    let gt = [0.3f32, 0.7, 0.3].repeat(w * h);
    let ids = vec![1u32; w * h];
    let views = vec![(c.clone(), w, h, gt, ids)];
    let (_gc, gp, _gs, _ga, cnt) = gradients(&[near.clone(), far.clone()], &views, [0.0, 0.0, 0.0]);
    let eps = 1e-3f32;
    for i in 0..2 {
        for a in [0usize, 2] {
            let mut up = [near.clone(), far.clone()];
            up[i].pos[a] += eps;
            let mut dn = [near.clone(), far.clone()];
            dn[i].pos[a] -= eps;
            let num = (loss_of(&up, &views) - loss_of(&dn, &views)) / (2.0 * f64::from(eps));
            let ana = gp[i][a];
            assert!(cnt[i] > 0);
            assert!(
                (num - ana).abs() < 0.05 * ana.abs().max(1.0),
                "splat {i} pos[{a}]: 数值 {num:.4} vs 解析 {ana:.4}"
            );
        }
    }
}

#[test]
fn five_splat_descent_reduces_loss() {
    // 多高斯重叠时，解析梯度与有限差分有偏差（离散的条目集/排序），
    // 但**方向必须能下降**：这里直接检查 20 步位置更新后损失变小。
    let c = cam();
    let (w, h) = (60usize, 60usize);
    let mut pts = Vec::new();
    for i in 0..5 {
        pts.push(Splat {
            pos: [0.02 * i as f32, 0.0, -1.8 + 0.1 * i as f32],
            scale: 0.07,
            color: [0.2 + 0.15 * i as f32, 0.8 - 0.1 * i as f32, 0.3],
            alpha: 0.6,
        });
    }
    let gt = [0.4f32, 0.5, 0.4].repeat(w * h);
    let ids = vec![1u32; w * h];
    let views = vec![(c.clone(), w, h, gt, ids)];
    let before = loss_of(&pts, &views);
    for _ in 0..50 {
        let (_gc, gp, _gs, _ga, cnt) = gradients(&pts, &views, [0.0, 0.0, 0.0]);
        for (i, p) in pts.iter_mut().enumerate() {
            if cnt[i] == 0 {
                continue;
            }
            let n = f64::from(cnt[i]);
            for (k, pk) in p.pos.iter_mut().enumerate() {
                *pk -= 5e-3 * (gp[i][k] / n) as f32;
            }
        }
    }
    let after = loss_of(&pts, &views);
    assert!(
        after < before * 0.99,
        "位置更新应降损（方向检查）：{before:.2} → {after:.2}"
    );
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
    let fine = build(
        std::slice::from_ref(&view),
        std::slice::from_ref(&rgb),
        0.01,
        2.0,
    );
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
