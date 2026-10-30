//! 光栅化器的判决性判据：覆盖率、透视校正、z 序、LOD 选择、wrap、图集、各向异性收益。

use trit_texture::convert::rgba_to_ttx2_mips;
use trit_texture::raster::math::Mat4;
use trit_texture::raster::{
    Derivative, Mesh, RenderOptions, SampleMode, Shaded, Stats, Target, Vertex, Wrap, rasterize,
    render_textured,
};
use trit_texture::sample::Rect;
use trit_texture::ttx2::{TtxDocument, from_ttx2};

// ---------- 构造工具 ----------

fn doc_from(px: Vec<u8>, width: usize, height: usize) -> TtxDocument {
    let bytes = rgba_to_ttx2_mips(&px, width, height).expect("encode");
    from_ttx2(&bytes).expect("parse")
}

/// 屏幕像素坐标 → 裁剪空间（`w = 1`，无透视）。
fn clip_at(px: f32, py: f32, z: f32, width: usize, height: usize) -> [f32; 4] {
    let ndc_x = px / width as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - py / height as f32 * 2.0;
    [ndc_x, ndc_y, z, 1.0]
}

/// 屏幕空间轴对齐四边形（两三角形），uv 从 `uv0` 线性到 `uv1`。
fn screen_quad(
    rect: (f32, f32, f32, f32),
    z: f32,
    uv: ([f32; 2], [f32; 2]),
    size: (usize, usize),
) -> Mesh {
    let (x0, y0, x1, y1) = rect;
    let (uv0, uv1) = uv;
    let (width, height) = size;
    let corner = |px, py, u, v| Vertex {
        clip: clip_at(px, py, z, width, height),
        uv: [u, v],
    };
    Mesh {
        vertices: vec![
            corner(x0, y0, uv0[0], uv0[1]),
            corner(x1, y0, uv1[0], uv0[1]),
            corner(x1, y1, uv1[0], uv1[1]),
            corner(x0, y1, uv0[0], uv1[1]),
        ],
        indices: vec![[0, 1, 2], [0, 2, 3]],
    }
}

/// 逐纹素哈希噪声：高频在 x、y 两个方向都有，缩小时第 0 级必走样。
fn noise(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
            let v = (h >> 13) as u8;
            px[o] = v;
            px[o + 1] = v.wrapping_mul(3);
            px[o + 2] = v.wrapping_mul(7);
            px[o + 3] = 255;
        }
    }
    px
}

/// 沿 u 的 32 纹素宽带 + 沿 v 的 2 纹素细线：地面足迹沿 v 很长，
/// 真值里 u 方向的宽带必须保留；三线性按长轴取 lod 会把 u 也糊掉。
fn bands(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let base = if (x / 32) % 2 == 0 { 200i32 } else { 55 };
            let fine = if (y / 2) % 2 == 0 { 25 } else { -25 };
            let c = (base + fine).clamp(0, 255) as u8;
            px[o] = c;
            px[o + 1] = c;
            px[o + 2] = c;
            px[o + 3] = 255;
        }
    }
    px
}

fn stripes(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let c = if (x / 2) % 2 == 0 { 235 } else { 20 };
            px[o] = c;
            px[o + 1] = c;
            px[o + 2] = c;
            px[o + 3] = 255;
        }
    }
    px
}

/// 透视地面（视图空间 y = -1）：与 `examples/render_scene.rs` 同一套几何。
fn floor_mesh(width: usize, height: usize, uv_scale: f32) -> Mesh {
    let proj = Mat4::perspective(60f32.to_radians(), width as f32 / height as f32, 0.1, 100.0);
    let corners = [(-2.0, -1.0), (2.0, -1.0), (2.0, -5.0), (-2.0, -5.0)];
    let vertices = corners
        .iter()
        .map(|&(x, z)| Vertex {
            clip: proj.mul_vec4([x, -1.0, z, 1.0]),
            uv: [(x + 2.0) / 4.0 * uv_scale, (z + 1.0) / -4.0 * uv_scale],
        })
        .collect();
    Mesh {
        vertices,
        indices: vec![[0, 1, 2], [0, 2, 3]],
    }
}

/// 把每个像素的 uv/导数记下来（不判深度之外的事，纯取证）。
fn capture(
    mesh: &Mesh,
    width: usize,
    height: usize,
) -> (Vec<Option<[f32; 2]>>, Vec<Option<Derivative>>) {
    let mut target = Target::new(width, height, [0, 0, 0]);
    let mut uvs = vec![None; width * height];
    let mut derivs = vec![None; width * height];
    rasterize(mesh, &mut target, |f| {
        let i = f.y * width + f.x;
        uvs[i] = Some(f.uv);
        derivs[i] = Some(f.derivative);
        Shaded::rgb([0, 0, 0])
    });
    (uvs, derivs)
}

// ---------- 1. 覆盖率 ----------

#[test]
fn quad_covered_exactly_once() {
    // 16×16 + 2 的幂坐标：顶点经视口变换能精确落回整数，45° 对角线才真正穿过像素中心，
    // 否则共享边上的像素根本没有 e == 0，top-left 规则测不到（12 高就会这样）。
    let (width, height) = (16usize, 16usize);
    let mesh = screen_quad(
        (2.0, 2.0, 10.0, 10.0),
        0.5,
        ([0.0, 0.0], [1.0, 1.0]),
        (width, height),
    );
    let mut target = Target::new(width, height, [0, 0, 0]);
    let mut counts = vec![0u32; width * height];
    let stats = rasterize(&mesh, &mut target, |f| {
        counts[f.y * width + f.x] += 1;
        Shaded::rgb([255, 0, 0])
    });
    // 像素中心在 x+0.5：四边形 [2,10)×[2,10) 覆盖 8×8 个像素。
    // 必须同时看 `covered`：共享对角线上两个三角形共面，重复覆盖会被深度测试挡掉，
    // 只看 `shaded` 测不出 top-left 规则是否生效。
    assert_eq!(stats.covered, 8 * 8, "覆盖像素数不对：{stats:?}");
    assert_eq!(stats.shaded, 8 * 8, "着色像素数不对：{stats:?}");
    for (i, &c) in counts.iter().enumerate() {
        let covered = {
            let (x, y) = (i % width, i / width);
            (2..10).contains(&x) && (2..10).contains(&y)
        };
        assert_eq!(
            c,
            u32::from(covered),
            "像素 {i} 着色 {c} 次，期望 {}",
            u32::from(covered)
        );
    }
}

// ---------- 2. z 序 ----------

#[test]
fn z_buffer_keeps_nearest_in_either_order() {
    let (width, height) = (8usize, 8usize);
    // 16×20 = 4×2 个块，每块一种纯色：编码后仍是可区分的纯色（红/绿/蓝/白）
    let (tex_w, tex_h) = (16usize, 20usize);
    let mut px = vec![0u8; tex_w * tex_h * 4];
    let colors = [[220, 20, 20], [20, 220, 20], [20, 20, 220], [220, 220, 220]];
    for y in 0..tex_h {
        for x in 0..tex_w {
            let o = (y * tex_w + x) * 4;
            let c = colors[x / 4];
            px[o..o + 3].copy_from_slice(&c);
            px[o + 3] = 255;
        }
    }
    let doc = doc_from(px, tex_w, tex_h);

    // 近处（z=0.4）取第 0 块（红），远处（z=0.8）取第 3 块（白）
    let build = |near_first: bool| {
        let mut mesh = Mesh::new();
        let mut quads = vec![(0.8f32, [0.9f32, 0.1]), (0.4, [0.1, 0.1])];
        if near_first {
            quads.reverse();
        }
        for (z, uv) in quads {
            let base = mesh.vertices.len() as u32;
            let quad = screen_quad((1.0, 1.0, 7.0, 7.0), z, (uv, uv), (width, height));
            mesh.vertices.extend(quad.vertices);
            for tri in quad.indices {
                mesh.indices
                    .push([tri[0] + base, tri[1] + base, tri[2] + base]);
            }
        }
        mesh
    };
    let opts = RenderOptions {
        width,
        height,
        mode: SampleMode::Nearest,
        wrap: Wrap::Clamp,
        ..Default::default()
    };

    let (image, stats) = render_textured(&build(false), &doc, &opts).expect("render");
    assert_eq!(stats.shaded, 2 * 6 * 6, "远的先写 36 个，近的再覆盖 36 个");
    let center = image.pixel(4, 4);
    assert!(
        center[0] > 150 && center[1] < 80,
        "中心应是被近处红色覆盖：{center:?}"
    );

    let (image, stats) = render_textured(&build(true), &doc, &opts).expect("render");
    assert_eq!(stats.shaded, 6 * 6, "近的先画时，远的不该再写任何像素");
    let center = image.pixel(4, 4);
    assert!(
        center[0] > 150 && center[1] < 80,
        "中心应仍是红色：{center:?}"
    );
}

// ---------- 3. 透视校正 ----------

/// 屏幕投影（与光栅化器同一套视口变换，但这里独立写一遍）。
fn project(clip: [f32; 4], width: usize, height: usize) -> (f64, f64) {
    let w = clip[3] as f64;
    let ndc_x = clip[0] as f64 / w;
    let ndc_y = clip[1] as f64 / w;
    (
        (ndc_x * 0.5 + 0.5) * width as f64,
        (0.5 - ndc_y * 0.5) * height as f64,
    )
}

#[test]
fn perspective_correct_uv_matches_ray_plane_intersection() {
    let (width, height) = (96usize, 72usize);
    let mesh = floor_mesh(width, height, 1.0);
    let (uvs, derivs) = capture(&mesh, width, height);

    // 屏幕空间仿射（错误做法）的 uv：用顶点屏幕位置做重心插值
    let p: Vec<(f64, f64)> = mesh
        .vertices
        .iter()
        .map(|v| project(v.clip, width, height))
        .collect();
    let tri = mesh.indices[0];
    let (a, b, c) = (p[tri[0] as usize], p[tri[1] as usize], p[tri[2] as usize]);
    let (ua, ub, uc) = (
        mesh.vertices[tri[0] as usize].uv,
        mesh.vertices[tri[1] as usize].uv,
        mesh.vertices[tri[2] as usize].uv,
    );
    let area = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);

    let fov = 60f32.to_radians();
    let tan_half = (fov * 0.5).tan() as f64;
    let aspect = width as f64 / height as f64;
    // 射线与平面 y = -1 求交 → 解析 uv（与光栅化器的插值代码完全独立的推导）
    let hit_uv = |px: f64, py: f64| -> Option<[f64; 2]> {
        let ndc_x = px / width as f64 * 2.0 - 1.0;
        let ndc_y = 1.0 - py / height as f64 * 2.0;
        let dir = [ndc_x * tan_half * aspect, ndc_y * tan_half, -1.0];
        if dir[1].abs() < 1e-3 {
            return None;
        }
        let t = -1.0 / dir[1];
        let (hx, hz) = (t * dir[0], t * dir[2]);
        Some([(hx + 2.0) / 4.0, (hz + 1.0) / -4.0])
    };
    let mut checked = 0;
    let mut worst_correct = 0f64;
    let mut worst_affine = 0f64;
    let (mut deriv_checked, mut worst_deriv) = (0usize, 0f64);
    for y in 0..height {
        for x in 0..width {
            let Some(uv) = uvs[y * width + x] else {
                continue;
            };
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let Some(want) = hit_uv(px, py) else {
                continue;
            };
            let err = (uv[0] as f64 - want[0])
                .abs()
                .max((uv[1] as f64 - want[1]).abs());
            worst_correct = worst_correct.max(err);
            checked += 1;

            // 导数：解析 uv 场的有限差分（每像素走一步）
            if let (Some(uxf), Some(uxb), Some(uyf), Some(uyb)) = (
                hit_uv(px + 1.0, py),
                hit_uv(px - 1.0, py),
                hit_uv(px, py + 1.0),
                hit_uv(px, py - 1.0),
            ) {
                let want_d = [
                    (uxf[0] - uxb[0]) * 0.5,
                    (uxf[1] - uxb[1]) * 0.5,
                    (uyf[0] - uyb[0]) * 0.5,
                    (uyf[1] - uyb[1]) * 0.5,
                ];
                if want_d.iter().all(|d| d.abs() < 2.0) {
                    let d = derivs[y * width + x].expect("captured");
                    let got_d = [d.du_x as f64, d.dv_x as f64, d.du_y as f64, d.dv_y as f64];
                    let e = (0..4)
                        .map(|k| (got_d[k] - want_d[k]).abs())
                        .fold(0.0, f64::max);
                    worst_deriv = worst_deriv.max(e);
                    deriv_checked += 1;
                }
            }

            let l1 = ((px - a.0) * (c.1 - a.1) - (py - a.1) * (c.0 - a.0)) / area;
            let l2 = ((b.0 - a.0) * (py - a.1) - (b.1 - a.1) * (px - a.0)) / area;
            let l0 = 1.0 - l1 - l2;
            if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                continue; // 仿射插值只对三角形 0 自身成立
            }
            let affine = [
                l0 * ua[0] as f64 + l1 * ub[0] as f64 + l2 * uc[0] as f64,
                l0 * ua[1] as f64 + l1 * ub[1] as f64 + l2 * uc[1] as f64,
            ];
            let affine_err = (affine[0] - want[0]).abs().max((affine[1] - want[1]).abs());
            worst_affine = worst_affine.max(affine_err);
        }
    }
    assert!(checked > 1000, "只有 {checked} 个像素被检查，用例没覆盖到");
    assert!(
        worst_correct < 1e-3,
        "透视校正的最大误差 {worst_correct} 太大"
    );
    assert!(deriv_checked > 500, "导数只校验了 {deriv_checked} 个像素");
    assert!(
        worst_deriv < 2e-3,
        "解析导数与有限差分的最大偏差 {worst_deriv} 太大"
    );
    assert!(
        worst_affine > 20.0 * 1e-3,
        "屏幕空间仿射插值的误差只有 {worst_affine} ⇒ 这个用例没有区分力"
    );
}

// ---------- 4. LOD 选择 ----------

#[test]
fn lod_follows_texels_per_pixel() {
    let (width, height) = (64usize, 64usize);
    let (tex_w, tex_h) = (256usize, 160usize);
    let doc = doc_from(stripes(tex_w, tex_h), tex_w, tex_h);

    // 铺满屏幕的四边形：uv 跨度 = 纹素/像素 × 屏幕像素 / 纹理纹素
    let render = |span: f32, mode: SampleMode| {
        let mesh = screen_quad(
            (0.0, 0.0, width as f32, height as f32),
            0.5,
            ([0.0, 0.0], [span, span * 0.625]),
            (width, height),
        );
        let opts = RenderOptions {
            width,
            height,
            mode,
            ..Default::default()
        };
        render_textured(&mesh, &doc, &opts).expect("render")
    };

    let (_, one_to_one) = render(64.0 / tex_w as f32, SampleMode::Auto);
    assert!(
        one_to_one.lod_mean.abs() < 0.05,
        "1 纹素/像素应 LOD≈0，实测 {:.3}",
        one_to_one.lod_mean
    );

    let (_, four_to_one) = render(256.0 / tex_w as f32, SampleMode::Auto);
    assert!(
        (four_to_one.lod_mean - 2.0).abs() < 0.05,
        "4 纹素/像素应 LOD≈2，实测 {:.3}",
        four_to_one.lod_mean
    );

    // mip 必须真的被用上：4:1 下三线性与「只取第 0 级」的图像应有明显差别。
    // 用噪声纹理——条纹在正好 4:1 时双线性会「碰巧」平均成正确值，没有区分力。
    let ndoc = doc_from(noise(tex_w, tex_h), tex_w, tex_h);
    let render_noise = |mode: SampleMode| {
        let mesh = screen_quad(
            (0.0, 0.0, width as f32, height as f32),
            0.5,
            (
                [0.0, 0.0],
                [256.0 / tex_w as f32, 256.0 / tex_w as f32 * 0.625],
            ),
            (width, height),
        );
        let opts = RenderOptions {
            width,
            height,
            mode,
            ..Default::default()
        };
        render_textured(&mesh, &ndoc, &opts).expect("render").0
    };
    let auto_img = render_noise(SampleMode::Auto);
    let level0_img = render_noise(SampleMode::Bilinear);
    let mean_diff: f64 = auto_img
        .pixels()
        .iter()
        .zip(level0_img.pixels())
        .map(|(a, b)| (a[0] as f64 - b[0] as f64).abs())
        .sum::<f64>()
        / auto_img.pixels().len() as f64;
    assert!(
        mean_diff > 20.0,
        "三线性与第 0 级几乎相同（{mean_diff:.2}）⇒ mip 没生效"
    );
}

// ---------- 5. wrap ----------

#[test]
fn repeat_is_periodic_clamp_is_not() {
    let (width, height) = (32usize, 32usize);
    let (tex_w, tex_h) = (64usize, 40usize);
    let doc = doc_from(stripes(tex_w, tex_h), tex_w, tex_h);
    let mesh_at = |offset: f32| {
        screen_quad(
            (0.0, 0.0, width as f32, height as f32),
            0.5,
            ([offset, offset], [offset + 0.5, offset + 0.5]),
            (width, height),
        )
    };
    let render = |offset: f32, wrap: Wrap| {
        let opts = RenderOptions {
            width,
            height,
            mode: SampleMode::Bilinear,
            wrap,
            ..Default::default()
        };
        render_textured(&mesh_at(offset), &doc, &opts)
            .expect("render")
            .0
    };

    let a = render(0.0, Wrap::Repeat);
    let b = render(1.0, Wrap::Repeat);
    // 相差整周期：纹素下标严格相同，但 f32 上 `(u+1)*W` 与 `u*W+W` 可能差 1 ulp，
    // 落在舍入边界上会差 1 LSB——判据取「逐通道 ≤1」，对照 clamp 的巨大差异仍然判决性十足。
    let worst = a
        .pixels()
        .iter()
        .zip(b.pixels())
        .map(|(p, q)| {
            (0..3)
                .map(|c| (p[c] as i32 - q[c] as i32).abs())
                .max()
                .unwrap()
        })
        .max()
        .unwrap();
    assert!(worst <= 1, "repeat 下整周期平移最大偏差 {worst} > 1 LSB");

    let c = render(0.0, Wrap::Clamp);
    let d = render(1.0, Wrap::Clamp);
    assert_ne!(c.pixels(), d.pixels(), "clamp 下平移应当完全不同");
}

// ---------- 6. 图集 ----------

#[test]
fn atlas_rect_never_leaks_neighbours() {
    let (tex_w, tex_h) = (64usize, 40usize);
    let rect = Rect::new(16, 10, 32, 20).expect("rect");
    let mut px = vec![0u8; tex_w * tex_h * 4];
    for y in 0..tex_h {
        for x in 0..tex_w {
            let o = (y * tex_w + x) * 4;
            let inside =
                x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height;
            let (r, g, b) = if inside {
                let c = if (x / 2) % 2 == 0 { 230 } else { 25 };
                (c, c, c)
            } else {
                (0, 0, 255)
            };
            px[o] = r;
            px[o + 1] = g;
            px[o + 2] = b;
            px[o + 3] = 255;
        }
    }
    let doc = doc_from(px, tex_w, tex_h);
    let (width, height) = (48usize, 48usize);
    let mesh = screen_quad(
        (0.0, 0.0, 48.0, 48.0),
        0.5,
        ([0.0, 0.0], [1.0, 1.0]),
        (width, height),
    );

    let render = |rect: Option<Rect>| {
        let opts = RenderOptions {
            width,
            height,
            mode: SampleMode::Aniso(8),
            rect,
            ..Default::default()
        };
        render_textured(&mesh, &doc, &opts).expect("render").0
    };
    let atlas = render(Some(rect));
    for (i, px) in atlas.pixels().iter().enumerate() {
        assert_eq!(px[0], px[1], "像素 {i} 灰度被破坏：{px:?}");
        assert_eq!(px[1], px[2], "像素 {i} 采到了矩形外的蓝：{px:?}");
    }
    let full = render(None);
    let leaked = full.pixels().iter().filter(|p| p[2] > p[0] + 8).count();
    assert!(leaked > 0, "不带矩形时一个蓝都没采到 ⇒ 用例没有区分力");
}

// ---------- 7. 各向异性收益 ----------

#[test]
fn aniso_beats_trilinear_on_oblique_floor() {
    let (width, height) = (96usize, 72usize);
    let (tex_w, tex_h) = (256usize, 160usize);
    let doc = doc_from(bands(tex_w, tex_h), tex_w, tex_h);
    let mesh = floor_mesh(width, height, 2.0);

    let render = |mode: SampleMode| {
        let opts = RenderOptions {
            width,
            height,
            mode,
            wrap: Wrap::Repeat,
            ..Default::default()
        };
        render_textured(&mesh, &doc, &opts).expect("render").0
    };
    let trilinear = render(SampleMode::Auto);
    let aniso = render(SampleMode::Aniso(8));

    // 真值 = 每个像素足迹上的积分。足迹以**像素中心**为中心，所以抖动取 [-0.5, 0.5)；
    // 用 [0, 1) 等于整体偏半像素，会把真值本身弄错。**抖动**采样（确定性 LCG）：
    // 等距采样在 27 纹素长的足迹上会与 2 纹素周期拍出摩尔纹。
    let (uvs, derivs) = capture(&mesh, width, height);
    const N: usize = 32;
    let mut sum_tri = 0f64;
    let mut sum_ani = 0f64;
    let mut count = 0usize;
    // 强各向异性像素（长短轴比 > 2）单独统计：近处足迹接近各向同性，两者本就一样好，
    // 混在一起会把差距摊薄，判据就失去了判决力。
    let (mut strong_tri, mut strong_ani, mut strong_n) = (0f64, 0f64, 0usize);
    for i in 0..width * height {
        let (Some(uv), Some(d)) = (uvs[i], derivs[i]) else {
            continue;
        };
        let mut state = 0x9e37_79b9_7f4a_7c15u64 ^ (i as u64).wrapping_mul(0x0100_0000_01b3);
        let mut next = || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 40) as f32) / ((1u64 << 24) as f32) - 0.5
        };
        let mut acc = 0f64;
        for _ in 0..N {
            for _ in 0..N {
                let su = next();
                let sv = next();
                let s = doc
                    .sample_trilinear_with(
                        uv[0] + d.du_x * su + d.du_y * sv,
                        uv[1] + d.dv_x * su + d.dv_y * sv,
                        0.0,
                        Wrap::Repeat,
                    )
                    .expect("sample");
                acc += s[0] as f64;
            }
        }
        let want = acc / (N * N) as f64;
        let e_tri = trilinear.pixels()[i][0] as f64 - want;
        let e_ani = aniso.pixels()[i][0] as f64 - want;
        sum_tri += e_tri * e_tri;
        sum_ani += e_ani * e_ani;
        count += 1;
        let tx = ((d.du_x * tex_w as f32).powi(2) + (d.dv_x * tex_h as f32).powi(2)).sqrt();
        let ty = ((d.du_y * tex_w as f32).powi(2) + (d.dv_y * tex_h as f32).powi(2)).sqrt();
        if tx.max(ty) > 2.0 * tx.min(ty) {
            strong_tri += e_tri * e_tri;
            strong_ani += e_ani * e_ani;
            strong_n += 1;
        }
    }
    let rmse_tri = (sum_tri / count as f64).sqrt();
    let rmse_ani = (sum_ani / count as f64).sqrt();
    let differing = trilinear
        .pixels()
        .iter()
        .zip(aniso.pixels())
        .filter(|(a, b)| a[0] != b[0])
        .count();
    assert!(
        differing * 20 > count,
        "两种模式只有 {differing} 个像素不同 ⇒ 各向异性没真的生效"
    );
    assert!(
        rmse_ani < rmse_tri * 0.8,
        "整体 RMSE：aniso {rmse_ani:.2} vs tri {rmse_tri:.2}"
    );
    assert!(
        strong_n > count / 4,
        "强各向异性像素只有 {strong_n}/{count}"
    );
    let rmse_strong_tri = (strong_tri / strong_n as f64).sqrt();
    let rmse_strong_ani = (strong_ani / strong_n as f64).sqrt();
    assert!(
        rmse_strong_ani * 2.0 < rmse_strong_tri,
        "强各向异性像素上 aniso {rmse_strong_ani:.2} 未优于 tri {rmse_strong_tri:.2} 的两倍"
    );
}

// ---------- 8. 统计口径 ----------

#[test]
fn stats_report_counts_and_lod() {
    let (width, height) = (16usize, 16usize);
    let doc = doc_from(stripes(64, 40), 64, 40);
    let mesh = screen_quad(
        (4.0, 4.0, 12.0, 12.0),
        0.5,
        ([0.0, 0.0], [0.25, 0.25]),
        (width, height),
    );
    let opts = RenderOptions {
        width,
        height,
        mode: SampleMode::Trilinear(1.25),
        ..Default::default()
    };
    let (_, stats): (_, Stats) = render_textured(&mesh, &doc, &opts).expect("render");
    assert_eq!(stats.triangles, 2);
    assert_eq!(stats.covered, 8 * 8);
    assert_eq!(stats.shaded, 8 * 8);
    assert!((stats.lod_mean - 1.25).abs() < 1e-6, "显式 lod 应原样上报");
    assert!((stats.lod_min - 1.25).abs() < 1e-6 && (stats.lod_max - 1.25).abs() < 1e-6);
    assert_eq!(stats.skipped_degenerate, 0);
    assert_eq!(stats.skipped_behind, 0);
}

#[test]
fn degenerate_and_behind_triangles_are_reported() {
    let mut mesh = Mesh::new();
    mesh.vertices = vec![
        Vertex {
            clip: [0.0, 0.0, 0.5, 1.0],
            uv: [0.0, 0.0],
        },
        Vertex {
            clip: [0.5, 0.0, 0.5, 1.0],
            uv: [1.0, 0.0],
        },
        Vertex {
            clip: [1.0, 0.0, 0.5, 1.0],
            uv: [1.0, 1.0],
        },
        // w = 0：需要裁剪，本实现不支持，必须计入 skipped_behind
        Vertex {
            clip: [0.0, 1.0, 0.5, 0.0],
            uv: [0.0, 0.0],
        },
        Vertex {
            clip: [1.0, 1.0, 0.5, 1.0],
            uv: [0.0, 0.0],
        },
        Vertex {
            clip: [-1.0, 1.0, 0.5, 1.0],
            uv: [0.0, 0.0],
        },
    ];
    mesh.indices = vec![[0, 1, 2], [3, 4, 5]];
    let mut target = Target::new(16, 16, [0, 0, 0]);
    let stats = rasterize(&mesh, &mut target, |_| Shaded::rgb([1, 2, 3]));
    assert_eq!(stats.triangles, 2);
    assert_eq!(stats.skipped_degenerate, 1, "共线三角形应计入退化");
    assert_eq!(stats.skipped_behind, 1, "w=0 的三角形应计入 skipped_behind");
    assert_eq!(stats.shaded, 0);
}
