//! GPU 管线出图：同一块透视地面，GPU 光栅 + 片元内自动 LOD，写到 PNG。
//!
//! `cargo run -p trit-gpu --features gpu --example render_scene_gpu -- out.png [cpu|gpu] [--compare]`
//!
//! - `cpu`：走 `trit_texture::raster` 的 CPU 参考；
//! - `gpu`（默认）：走 [`trit_gpu::render::GpuRenderer`]；
//! - `--compare`：两种都渲，打印逐像素差并额外写一张 `<out>_cpu.png`。

use std::path::{Path, PathBuf};

use trit_gpu::render::{GpuRenderer, Mesh, Vertex};
use trit_gpu::sampler::{MipLevel, Wrap as GpuWrap};
use trit_texture::convert::{RgbaImage, build_mip_chain, rgba_to_ttx2_mips};
use trit_texture::pngio::save_png_rgba;
use trit_texture::raster::math::Mat4;
use trit_texture::raster::{
    Mesh as CpuMesh, RenderOptions, SampleMode, Vertex as CpuVertex, Wrap as CpuWrap,
    render_textured,
};
use trit_texture::ttx2::{TtxDocument, from_ttx2};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 384;

/// 程序化贴图：宽条纹（沿 u）+ 细线（沿 v）+ 色块 + 明暗渐变。
fn floor_texture(width: usize, height: usize) -> Vec<u8> {
    let mut px = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let stripe = if (x / 8) % 2 == 0 { 235 } else { 40 };
            let patch = ((x / 32) * 7 + (y / 32) * 13) % 5;
            let tint = [(0, 0, 0), (60, 0, 0), (0, 60, 0), (0, 0, 60), (50, 50, 0)][patch];
            let fade = (y * 255 / height) as i32;
            let base = (stripe + fade - 128).clamp(0, 255) as u8;
            px[o] = base.saturating_add(tint.0);
            px[o + 1] = base.saturating_add(tint.1);
            px[o + 2] = base.saturating_add(tint.2);
            px[o + 3] = 255;
        }
    }
    px
}

/// 视图空间 y = -1 的地面，透视投影后转裁剪空间（与 CPU 示例同一套几何）。
fn floor_mesh() -> (Mesh, CpuMesh) {
    let proj = Mat4::perspective(60f32.to_radians(), WIDTH as f32 / HEIGHT as f32, 0.1, 100.0);
    let tiles = 2.0f32;
    let corners = [(-2.0, -1.0), (2.0, -1.0), (2.0, -5.0), (-2.0, -5.0)];
    let vertices: Vec<Vertex> = corners
        .iter()
        .map(|&(x, z)| {
            let p = proj.mul_vec4([x, -1.0, z, 1.0]);
            Vertex {
                clip: p,
                uv: [(x + 2.0) / 4.0 * tiles, (z + 1.0) / -4.0 * tiles],
            }
        })
        .collect();
    let indices = vec![[0u32, 1, 2], [0, 2, 3]];
    let cpu = CpuMesh {
        vertices: vertices
            .iter()
            .map(|v| CpuVertex {
                clip: v.clip,
                uv: v.uv,
            })
            .collect(),
        indices: indices.clone(),
    };
    (Mesh { vertices, indices }, cpu)
}

fn write_png(path: &Path, rgb: &[u8], width: usize, height: usize) {
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in rgb.as_chunks::<3>().0 {
        rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
    }
    save_png_rgba(
        path,
        &RgbaImage {
            width,
            height,
            rgba,
        },
    )
    .expect("png");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "render_scene_gpu.png".into()));
    let mut which = "gpu".to_string();
    let mut compare = false;
    for arg in args {
        if arg == "--compare" {
            compare = true;
        } else {
            which = arg;
        }
    }

    let (tex_w, tex_h) = (256usize, 160usize);
    let pixels = floor_texture(tex_w, tex_h);
    let bytes = rgba_to_ttx2_mips(&pixels, tex_w, tex_h).expect("encode");
    let doc: TtxDocument = from_ttx2(&bytes).expect("parse");
    let chain = build_mip_chain(&pixels, tex_w, tex_h).expect("mip chain");
    let flat: Vec<Vec<u8>> = chain.iter().map(|t| t.blocks.concat()).collect();
    let levels: Vec<MipLevel> = chain
        .iter()
        .zip(&flat)
        .map(|(t, blocks)| MipLevel {
            blocks,
            width: t.width,
            height: t.height,
            rect: None,
        })
        .collect();

    let (mesh, cpu_mesh) = floor_mesh();
    let render_cpu = || {
        let opts = RenderOptions {
            width: WIDTH as usize,
            height: HEIGHT as usize,
            mode: SampleMode::Auto,
            wrap: CpuWrap::Repeat,
            background: [12, 12, 16],
            ..Default::default()
        };
        let (image, stats) = render_textured(&cpu_mesh, &doc, &opts).expect("cpu render");
        let mut rgb = Vec::with_capacity(WIDTH as usize * HEIGHT as usize * 3);
        for px in image.pixels() {
            rgb.extend_from_slice(px);
        }
        (rgb, stats.shaded)
    };

    if which == "cpu" {
        let (rgb, shaded) = render_cpu();
        write_png(&out, &rgb, WIDTH as usize, HEIGHT as usize);
        println!("{}: CPU 参考，着色 {shaded} 像素", out.display());
        return;
    }

    let Some(renderer) = GpuRenderer::new(&levels, GpuWrap::Repeat) else {
        eprintln!("没有可用的 GPU adapter");
        return;
    };
    let Some(rgb) = renderer.render(&mesh, WIDTH, HEIGHT, [12, 12, 16], None) else {
        eprintln!("GPU 渲染失败");
        return;
    };
    write_png(&out, &rgb, WIDTH as usize, HEIGHT as usize);
    println!(
        "{}: GPU 管线（{} 级 mip），{} 字节 RGB",
        out.display(),
        renderer.level_count(),
        rgb.len()
    );

    if compare {
        let (cpu_rgb, shaded) = render_cpu();
        let mut sum = 0f64;
        let mut worst = 0i32;
        let mut over_one = 0usize;
        for (a, b) in rgb
            .as_chunks::<3>()
            .0
            .iter()
            .zip(cpu_rgb.as_chunks::<3>().0)
        {
            let d = (0..3)
                .map(|c| (a[c] as i32 - b[c] as i32).abs())
                .max()
                .unwrap();
            sum += d as f64;
            worst = worst.max(d);
            if d > 1 {
                over_one += 1;
            }
        }
        let n = (WIDTH * HEIGHT) as f64;
        println!(
            "对拍 CPU：均值 {:.4}，最大 {}，>1 LSB {:.2}%（CPU 着色 {shaded} 像素）",
            sum / n,
            worst,
            over_one as f64 * 100.0 / n
        );
        let cpu_out = out.with_file_name(format!(
            "{}_cpu.{}",
            out.file_stem().unwrap_or_default().to_string_lossy(),
            out.extension().unwrap_or_default().to_string_lossy()
        ));
        write_png(&cpu_out, &cpu_rgb, WIDTH as usize, HEIGHT as usize);
        println!("{}: CPU 参考", cpu_out.display());
    }
}
