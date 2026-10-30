//! 用压缩域纹理渲染一块透视地面，写出 PNG。
//!
//! `cargo run -p trit-texture --features png --example render_scene -- out.png [mode]`
//!
//! `mode`：`nearest` / `bilinear` / `auto` / `aniso[:taps]`（默认 `aniso:8`）。
//! 地面从近到远跨越 4 倍的纹素/像素比，mip 与各向异性的差别肉眼可见。

use std::path::PathBuf;

use trit_texture::convert::{RgbaImage, rgba_to_ttx2_mips};
use trit_texture::pngio::save_png_rgba;
use trit_texture::raster::math::Mat4;
use trit_texture::raster::{Mesh, RenderOptions, SampleMode, Vertex, Wrap, render_textured};
use trit_texture::ttx2::from_ttx2;

/// 程序化贴图：细条纹 + 粗色块 + 明暗渐变（缩小时条纹最先糊，色块还能分辨）。
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

fn parse_mode(s: &str) -> SampleMode {
    match s {
        "nearest" => SampleMode::Nearest,
        "bilinear" => SampleMode::Bilinear,
        "auto" => SampleMode::Auto,
        other => {
            let taps = other
                .strip_prefix("aniso:")
                .and_then(|n| n.parse().ok())
                .unwrap_or(8u32);
            SampleMode::Aniso(taps)
        }
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(args.next().unwrap_or_else(|| "render_scene.png".into()));
    let mode = parse_mode(&args.next().unwrap_or_else(|| "aniso:8".into()));

    let (tex_w, tex_h) = (256usize, 160usize);
    let bytes = rgba_to_ttx2_mips(&floor_texture(tex_w, tex_h), tex_w, tex_h).expect("encode");
    let doc = from_ttx2(&bytes).expect("parse");

    let (width, height) = (512usize, 384usize);
    let proj = Mat4::perspective(60f32.to_radians(), width as f32 / height as f32, 0.1, 100.0);
    let tiles = 2.0f32;
    // 地面：视图空间 y = -1，x ∈ [-2, 2]，z ∈ [-1, -5]（相机在原点看向 -z）
    let corners = [(-2.0, -1.0), (2.0, -1.0), (2.0, -5.0), (-2.0, -5.0)];
    let vertices = corners
        .iter()
        .map(|&(x, z)| Vertex {
            clip: proj.mul_vec4([x, -1.0, z, 1.0]),
            uv: [(x + 2.0) / 4.0 * tiles, (z + 1.0) / -4.0 * tiles],
        })
        .collect();
    let mesh = Mesh {
        vertices,
        indices: vec![[0, 1, 2], [0, 2, 3]],
    };

    let opts = RenderOptions {
        width,
        height,
        mode,
        wrap: Wrap::Repeat,
        background: [12, 12, 16],
        ..Default::default()
    };
    let (image, stats) = render_textured(&mesh, &doc, &opts).expect("render");

    let rgba = RgbaImage {
        width: image.width(),
        height: image.height(),
        rgba: image.to_rgba(),
    };
    save_png_rgba(&out, &rgba).expect("png");
    println!(
        "{}: {width}x{height} mode={mode:?} 三角形 {} 覆盖 {} 着色 {} LOD {:.2}..{:.2}（均值 {:.2}）",
        out.display(),
        stats.triangles,
        stats.covered,
        stats.shaded,
        stats.lod_min,
        stats.lod_max,
        stats.lod_mean
    );
}
