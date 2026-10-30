//! GPU 光栅管线与 CPU 参考对拍。
//!
//! 判据分两档：
//! - `force_lod`：两边采样参数完全相同 ⇒ 要求逐像素有界相等（每通道 ≤1 LSB，越界率 ≤1%）；
//! - `Auto`：LOD 来源不同（GPU 是 2×2 quad 有限差分，CPU 是解析式）⇒ 只做有界对拍，
//!   并且必须能区分 mip 是否生效（对比「只取第 0 级」的 CPU 结果）。
//!
//! 没有可用 adapter 时跳过（与 `gpu::run` 同约定：返回 `None` 直接 `return`）。

#![cfg(feature = "gpu")]

use trit_gpu::render::{GpuRenderer, Mesh, Vertex};
use trit_gpu::sampler::{MipLevel, Wrap as GpuWrap};
use trit_texture::convert::build_mip_chain;
use trit_texture::raster::{
    Mesh as CpuMesh, RenderOptions as CpuOptions, SampleMode, Vertex as CpuVertex, Wrap as CpuWrap,
    render_textured,
};
use trit_texture::ttx2::TtxDocument;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;

/// 确定性纹理：渐变 + 条纹 + 椒盐，三种调色板槽位都会用到。
fn synthetic(width: usize, height: usize) -> Vec<u8> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut pixels = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let noise = (state >> 56) as u8 / 24;
            let stripe = if (x / 8) % 2 == 0 { 30 } else { 0 };
            let o = (y * width + x) * 4;
            pixels[o] = ((x * 255 / width) as u8)
                .saturating_add(stripe)
                .saturating_add(noise);
            pixels[o + 1] = ((y * 255 / height) as u8).saturating_add(noise);
            pixels[o + 2] = (((x + y) * 255 / (width + height)) as u8).wrapping_add(noise);
            pixels[o + 3] = 255;
        }
    }
    pixels
}

/// 一条 mip 链 → (文档, 展平后的块流, GPU 级描述符)。
struct Fixture {
    doc: TtxDocument,
    flat: Vec<Vec<u8>>,
}

impl Fixture {
    fn levels(&self) -> Vec<MipLevel<'_>> {
        self.doc
            .levels()
            .expect("texture")
            .iter()
            .zip(&self.flat)
            .map(|(t, blocks)| MipLevel {
                blocks,
                width: t.width,
                height: t.height,
                rect: None,
            })
            .collect()
    }
}

fn fixture(width: usize, height: usize, pixels: Vec<u8>) -> Fixture {
    let chain = build_mip_chain(&pixels, width, height).expect("mip chain");
    let flat: Vec<Vec<u8>> = chain.iter().map(|t| t.blocks.concat()).collect();
    Fixture {
        doc: TtxDocument::Texture { levels: chain },
        flat,
    }
}

fn synthetic_fixture() -> Fixture {
    fixture(256, 160, synthetic(256, 160))
}

/// 铺满屏幕的四边形；`uv` 从 `(0,0)`（左上）线性到 `uv1`。
fn screen_quad(uv1: [f32; 2], z: f32) -> Mesh {
    let clip = |px: f32, py: f32| {
        [
            px / WIDTH as f32 * 2.0 - 1.0,
            1.0 - py / HEIGHT as f32 * 2.0,
            z,
            1.0,
        ]
    };
    Mesh {
        vertices: vec![
            Vertex {
                clip: clip(0.0, 0.0),
                uv: [0.0, 0.0],
            },
            Vertex {
                clip: clip(WIDTH as f32, 0.0),
                uv: [uv1[0], 0.0],
            },
            Vertex {
                clip: clip(WIDTH as f32, HEIGHT as f32),
                uv: [uv1[0], uv1[1]],
            },
            Vertex {
                clip: clip(0.0, HEIGHT as f32),
                uv: [0.0, uv1[1]],
            },
        ],
        indices: vec![[0, 1, 2], [0, 2, 3]],
    }
}

fn to_cpu(mesh: &Mesh) -> CpuMesh {
    CpuMesh {
        vertices: mesh
            .vertices
            .iter()
            .map(|v| CpuVertex {
                clip: v.clip,
                uv: v.uv,
            })
            .collect(),
        indices: mesh.indices.clone(),
    }
}

fn cpu_render(fixture: &Fixture, mesh: &Mesh, mode: SampleMode) -> Vec<[u8; 3]> {
    let opts = CpuOptions {
        width: WIDTH as usize,
        height: HEIGHT as usize,
        mode,
        wrap: CpuWrap::Repeat,
        rect: None,
        background: [0, 0, 0],
    };
    render_textured(&to_cpu(mesh), &fixture.doc, &opts)
        .expect("cpu render")
        .0
        .pixels()
        .to_vec()
}

fn gpu_render(renderer: &GpuRenderer, mesh: &Mesh, force_lod: Option<f32>) -> Vec<[u8; 3]> {
    let rgb = renderer
        .render(mesh, WIDTH, HEIGHT, [0, 0, 0], force_lod)
        .expect("gpu render");
    rgb.as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0], c[1], c[2]])
        .collect()
}

struct Diff {
    mean: f64,
    worst: i32,
    over_one: usize,
    total: usize,
}

fn diff(a: &[[u8; 3]], b: &[[u8; 3]]) -> Diff {
    let mut sum = 0f64;
    let mut worst = 0i32;
    let mut over_one = 0usize;
    for (p, q) in a.iter().zip(b) {
        let d = (0..3)
            .map(|c| (p[c] as i32 - q[c] as i32).abs())
            .max()
            .unwrap();
        sum += d as f64;
        worst = worst.max(d);
        if d > 1 {
            over_one += 1;
        }
    }
    Diff {
        mean: sum / a.len() as f64,
        worst,
        over_one,
        total: a.len(),
    }
}

#[test]
fn forced_lod_matches_cpu_reference() {
    let fixture = synthetic_fixture();
    let Some(renderer) = GpuRenderer::new(&fixture.levels(), GpuWrap::Repeat) else {
        return;
    };
    let mesh = screen_quad([0.5, 0.5], 0.5);
    let gpu = gpu_render(&renderer, &mesh, Some(1.25));
    let cpu = cpu_render(&fixture, &mesh, SampleMode::Trilinear(1.25));
    let d = diff(&gpu, &cpu);
    assert!(
        d.over_one * 100 <= d.total,
        "每通道差 >1 LSB 的像素占 {:.2}%（上限 1%），最大差 {}，均值 {:.3}",
        d.over_one as f64 * 100.0 / d.total as f64,
        d.worst,
        d.mean
    );
    assert!(d.mean < 0.5, "平均差 {:.3} 太大", d.mean);
}

#[test]
fn auto_lod_is_close_and_actually_uses_mips() {
    let fixture = synthetic_fixture();
    let Some(renderer) = GpuRenderer::new(&fixture.levels(), GpuWrap::Repeat) else {
        return;
    };
    // 铺满屏幕、uv 跨整张 256×160 纹理 ⇒ 约 4 纹素/像素，LOD 应≈2
    let mesh = screen_quad([1.0, 1.0], 0.5);
    let gpu = gpu_render(&renderer, &mesh, None);
    let cpu_auto = cpu_render(&fixture, &mesh, SampleMode::Auto);
    let cpu_level0 = cpu_render(&fixture, &mesh, SampleMode::Bilinear);

    let d_auto = diff(&gpu, &cpu_auto);
    assert!(
        d_auto.mean < 2.5,
        "GPU Auto 与 CPU Auto 平均差 {:.3} 太大（最大 {}，>1 LSB 占 {:.1}%）",
        d_auto.mean,
        d_auto.worst,
        d_auto.over_one as f64 * 100.0 / d_auto.total as f64
    );
    assert!(
        d_auto.over_one * 20 <= d_auto.total,
        "差 >1 LSB 的像素占 {:.1}%",
        d_auto.over_one as f64 * 100.0 / d_auto.total as f64
    );

    // mip 必须真的生效：与「只取第 0 级」的 CPU 结果相比应当明显更远
    let d_level0 = diff(&gpu, &cpu_level0);
    assert!(
        d_level0.mean > 3.0 * d_auto.mean.max(0.2),
        "GPU 结果更像第 0 级（均值 {:.2} vs {:.2}）⇒ mip 没生效",
        d_level0.mean,
        d_auto.mean
    );
}

#[test]
fn depth_test_keeps_near_quad() {
    let (tex_w, tex_h) = (16usize, 20usize);
    let colors = [[220, 20, 20], [20, 220, 20], [20, 20, 220], [220, 220, 220]];
    let mut px = vec![0u8; tex_w * tex_h * 4];
    for y in 0..tex_h {
        for x in 0..tex_w {
            let o = (y * tex_w + x) * 4;
            px[o..o + 3].copy_from_slice(&colors[x / 4]);
            px[o + 3] = 255;
        }
    }
    let fixture = fixture(tex_w, tex_h, px);
    let Some(renderer) = GpuRenderer::new(&fixture.levels(), GpuWrap::Clamp) else {
        return;
    };

    let quad = |z: f32, uv: [f32; 2]| {
        screen_quad([0.0, 0.0], z)
            .vertices
            .iter()
            .map(|v| Vertex { clip: v.clip, uv })
            .collect::<Vec<_>>()
    };
    // 远（z=0.8，白色块）先画，近（z=0.4，红色块）后画
    let mesh = Mesh {
        vertices: {
            let mut v = quad(0.8, [0.9, 0.1]);
            v.extend(quad(0.4, [0.1, 0.1]));
            v
        },
        indices: vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]],
    };
    let center = (HEIGHT as usize / 2) * WIDTH as usize + WIDTH as usize / 2;
    let gpu = gpu_render(&renderer, &mesh, Some(0.0));
    let near = gpu[center];
    assert!(
        near[0] > 150 && near[1] < 80,
        "中心应是近处红色，实测 {near:?}"
    );

    // 反过来画：深度测试同样要挡住远的
    let mesh_rev = Mesh {
        vertices: mesh.vertices.clone(),
        indices: vec![[4, 5, 6], [4, 6, 7], [0, 1, 2], [0, 2, 3]],
    };
    let gpu = gpu_render(&renderer, &mesh_rev, Some(0.0));
    let near = gpu[center];
    assert!(
        near[0] > 150 && near[1] < 80,
        "反序绘制后中心仍应是近处红色，实测 {near:?}"
    );
}

#[test]
fn v_axis_is_not_flipped() {
    let (tex_w, tex_h) = (16usize, 20usize);
    let mut px = vec![0u8; tex_w * tex_h * 4];
    for y in 0..tex_h {
        for x in 0..tex_w {
            let o = (y * tex_w + x) * 4;
            let top = y < tex_h / 2;
            px[o..o + 3].copy_from_slice(if top { &[220, 20, 20] } else { &[20, 20, 220] });
            px[o + 3] = 255;
        }
    }
    let fixture = fixture(tex_w, tex_h, px);
    let Some(renderer) = GpuRenderer::new(&fixture.levels(), GpuWrap::Clamp) else {
        return;
    };
    // 半张纹理铺满屏幕：v=0（红）在上，v=1（蓝）在下
    let mesh = screen_quad([1.0, 1.0], 0.5);
    let gpu = gpu_render(&renderer, &mesh, Some(0.0));
    let top = gpu[WIDTH as usize + WIDTH as usize / 2];
    let bottom = gpu[(HEIGHT as usize - 2) * WIDTH as usize + WIDTH as usize / 2];
    assert!(top[0] > 150 && top[2] < 80, "顶部应取 v=0 的红：{top:?}");
    assert!(
        bottom[2] > 150 && bottom[0] < 80,
        "底部应取 v=1 的蓝：{bottom:?}"
    );
}

#[test]
fn rejects_bad_input() {
    let fixture = synthetic_fixture();
    let Some(renderer) = GpuRenderer::new(&fixture.levels(), GpuWrap::Repeat) else {
        return;
    };
    let mesh = screen_quad([1.0, 1.0], 0.5);
    assert!(renderer.render(&mesh, 0, HEIGHT, [0, 0, 0], None).is_none());
    assert!(
        renderer
            .render(&Mesh::default(), WIDTH, HEIGHT, [0, 0, 0], None)
            .is_none()
    );
    let bad = Mesh {
        vertices: mesh.vertices.clone(),
        indices: vec![[0, 1, 99]],
    };
    assert!(
        renderer
            .render(&bad, WIDTH, HEIGHT, [0, 0, 0], None)
            .is_none(),
        "越界索引必须拒绝，不能交给 GPU 读"
    );
    // 形状不符的级（宽不是 4 的倍数）在构造期就该拒绝
    assert!(
        GpuRenderer::new(
            &[MipLevel {
                blocks: &[],
                width: 5,
                height: 10,
                rect: None
            }],
            GpuWrap::Clamp
        )
        .is_none()
    );
}
