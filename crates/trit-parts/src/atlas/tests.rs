use crate::mesh::Mesh;

use super::*;
fn cube() -> Mesh {
    let v = vec![
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    let t = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    Mesh { verts: v, tris: t }
}

#[test]
fn atlas_uvs_are_normalized_and_charts_do_not_overlap() {
    let m = cube();
    let tex = vec![0.5f32; 32 * 32 * 3];
    let a = bake_atlas(&m, &tex, 32, 32, 256, 60.0, 45.0, false, None).unwrap();
    assert_eq!(a.uv.len(), m.tris.len());
    for tri in &a.uv {
        for p in tri {
            assert!(
                (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]),
                "UV 必须落在 [0,1]，实际 {p:?}"
            );
        }
    }
    // 图集矩形两两不重叠
    for i in 0..a.charts.len() {
        for j in i + 1..a.charts.len() {
            let (a1, b1) = (a.charts[i].rect, a.charts[j].rect);
            let ox = a1[0] < b1[0] + b1[2] && b1[0] < a1[0] + a1[2];
            let oy = a1[1] < b1[1] + b1[3] && b1[1] < a1[1] + a1[3];
            assert!(!(ox && oy), "chart 矩形重叠：{a1:?} vs {b1:?}");
        }
    }
    assert!(
        a.charts.len() >= 6,
        "立方体至少 6 个 chart，实际 {}",
        a.charts.len()
    );
}

#[test]
fn atlas_bakes_a_solid_texture_into_solid_pixels() {
    let m = cube();
    let (tw, th) = (16usize, 16usize);
    let tex: Vec<f32> = (0..tw * th).flat_map(|_| [0.25, 0.5, 0.75]).collect();
    let a = bake_atlas(&m, &tex, tw, th, 128, 40.0, 45.0, false, None).unwrap();
    // 所有被覆盖的纹素都必须等于源色（padding 处是 0，用 >0 判定被覆盖）
    let mut covered = 0usize;
    for px in a.img.chunks(3) {
        if px[0] > 0.0 || px[1] > 0.0 || px[2] > 0.0 {
            covered += 1;
            assert!(
                (px[0] - 0.25).abs() < 1e-6
                    && (px[1] - 0.5).abs() < 1e-6
                    && (px[2] - 0.75).abs() < 1e-6,
                "烘出来的颜色应等于源色，实际 {px:?}"
            );
        }
    }
    assert!(covered > 500, "图集应被覆盖，实际 {covered} 个纹素");
}

#[test]
fn flat_surface_has_uniform_texel_density() {
    // 两个直角三角组成的平面：同一 chart、同一投影 ⇒ 密度应几乎相同
    let m = Mesh {
        verts: vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ],
        tris: vec![[0, 1, 2], [0, 2, 3]],
    };
    let tex = vec![0.5f32; 8 * 8 * 3];
    let a = bake_atlas(&m, &tex, 8, 8, 128, 50.0, 45.0, false, None).unwrap();
    let st = uv_stats(&m, &a);
    assert_eq!(st.charts, 1, "共面应只出一个 chart");
    assert!(
        (st.spread - 1.0).abs() < 0.02,
        "共面密度应均匀，实际 spread {:.3}",
        st.spread
    );
}

fn cylinder_patch(nu: usize, nv: usize) -> Mesh {
    // 半圆柱面（弧 180°、半径 1、高 1）：平面投影会在两侧压缩，LSCM 应更均匀
    let mut verts = Vec::new();
    for j in 0..=nv {
        for i in 0..=nu {
            let th = std::f32::consts::PI * i as f32 / nu as f32;
            verts.push([th.cos(), j as f32 / nv as f32, th.sin()]);
        }
    }
    let mut tris = Vec::new();
    for j in 0..nv {
        for i in 0..nu {
            let a = (j * (nu + 1) + i) as u32;
            let b = (j * (nu + 1) + i + 1) as u32;
            let c = ((j + 1) * (nu + 1) + i) as u32;
            let d = ((j + 1) * (nu + 1) + i + 1) as u32;
            tris.push([a, b, c]);
            tris.push([b, d, c]);
        }
    }
    Mesh { verts, tris }
}

#[test]
fn lscm_flattens_a_curved_patch_better_than_planar_projection() {
    let m = cylinder_patch(32, 8);
    let tex = vec![0.5f32; 8 * 8 * 3];
    let planar = bake_atlas(&m, &tex, 8, 8, 512, 0.0, 89.0, false, None).unwrap();
    let lscm = bake_atlas(&m, &tex, 8, 8, 512, 0.0, 89.0, true, None).unwrap();
    let sp = uv_stats(&m, &planar);
    let sl = uv_stats(&m, &lscm);
    assert_eq!(
        sp.charts, 1,
        "半圆柱应是一个 chart（阈值 89°），实际 {}",
        sp.charts
    );
    assert_eq!(sl.charts, 1);
    assert!(
        sl.spread < 3.0,
        "LSCM 后密度应接近均匀，实际 spread {:.2}",
        sl.spread
    );
    assert!(
        sl.spread < sp.spread * 0.6,
        "LSCM 应显著优于平面投影：planar {:.2} vs lscm {:.2}",
        sp.spread,
        sl.spread
    );
}

#[test]
fn per_class_styles_only_affect_their_own_triangles() {
    // 同一平面两个三角形：第 0 个强制灰度，第 1 个不动
    let m = Mesh {
        verts: vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        tris: vec![[0, 1, 2], [0, 2, 3]],
    };
    // 彩色源（R 通道随 x 变化）
    let (tw, th) = (32usize, 32usize);
    let tex: Vec<f32> = (0..tw * th)
        .flat_map(|i| {
            let x = (i % tw) as f32 / tw as f32;
            [x, 0.5, 0.5]
        })
        .collect();
    let gray = crate::tex::Style {
        sat: 0.0,
        ..Default::default()
    };
    let id = crate::tex::Style::default();
    let styles = [gray, id];
    let class = [0u8, 1u8];
    let a = bake_atlas(
        &m,
        &tex,
        tw,
        th,
        128,
        50.0,
        45.0,
        false,
        Some(&PerClass {
            class: &class,
            styles: &styles,
        }),
    )
    .unwrap();
    let at = |fi: usize| -> [f32; 3] {
        // 取该三角形 UV 质心处的图集像素
        let u = (a.uv[fi][0][0] + a.uv[fi][1][0] + a.uv[fi][2][0]) / 3.0;
        let v = (a.uv[fi][0][1] + a.uv[fi][1][1] + a.uv[fi][2][1]) / 3.0;
        let x = (u * a.w as f32) as usize;
        let y = ((1.0 - v) * a.h as f32) as usize;
        let i = (y.min(a.h - 1) * a.w + x.min(a.w - 1)) * 3;
        [a.img[i], a.img[i + 1], a.img[i + 2]]
    };
    let c0 = at(0);
    let c1 = at(1);
    assert!(
        (c0[0] - c0[1]).abs() < 0.02 && (c0[1] - c0[2]).abs() < 0.02,
        "类别 0 应为灰度，实际 {c0:?}"
    );
    assert!(
        (c1[0] - c1[1]).abs() > 0.05,
        "类别 1 应保留彩色，实际 {c1:?}"
    );
    // 精确判据：只有类别 0 的纹素被改写
    assert!(a.styled_texels[0] > 0, "类别 0 应有纹素被改写");
    assert_eq!(a.styled_texels[1], 0, "类别 1（恒等）不应改写任何纹素");
}

#[test]
fn atlas_is_deterministic() {
    let m = cube();
    let tex = vec![0.3f32; 16 * 16 * 3];
    let a = bake_atlas(&m, &tex, 16, 16, 128, 40.0, 45.0, false, None).unwrap();
    let b = bake_atlas(&m, &tex, 16, 16, 128, 40.0, 45.0, false, None).unwrap();
    assert_eq!(a.uv, b.uv);
    assert_eq!(a.img, b.img);
    assert_eq!(a.charts.len(), b.charts.len());
}
