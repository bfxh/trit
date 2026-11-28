//! 单帧落盘：RGB PNG 与可选 16-bit 深度 PNG。
//! 深度 PNG 为灰度 16-bit，按 `--depth-max` 线性量化（0..65535）；
//! 未覆盖背景与超出 range 的远点都饱和到 65535。
use std::path::{Path, PathBuf};
use trit_splat::camera::Transforms;
use trit_splat::color::ColorConfig;
use trit_splat::ply::RawGaussian;
use trit_splat::raster::{render, render_with_depth};

use crate::cli;

#[allow(clippy::too_many_arguments)]
pub(super) fn render_slot(
    gaussians: &[RawGaussian],
    transforms: &Transforms,
    min_scales: Option<&[f32]>,
    dataset: &str,
    bg_mode: &str,
    want_psnr: bool,
    want_depth: bool,
    depth_max: f32,
    color: &ColorConfig,
    frame_idx: usize,
    path: &Path,
) -> Result<Option<f64>, String> {
    let cam = &transforms.frames[frame_idx];
    let background = cli::background(bg_mode, dataset, &cam.file_path);
    let mut image = if want_depth {
        let (image, depth) = render_with_depth(
            gaussians,
            &cam.c2w,
            &transforms.intrinsics,
            background,
            min_scales,
        );
        let depth_path = depth_path(path);
        write_depth_png(&depth_path, image.width, image.height, &depth, depth_max)?;
        println!("  depth -> {}", depth_path.display());
        image
    } else {
        render(
            gaussians,
            &cam.c2w,
            &transforms.intrinsics,
            background,
            min_scales,
        )
    };
    color.apply(&mut image);
    cli::write_png(path, &image).map_err(|e| format!("写 PNG: {e}"))?;
    if !want_psnr {
        return Ok(None);
    }
    let gt_path = Path::new(dataset).join(&cam.file_path);
    let Some(gt) = cli::decode_gt_rgb(&gt_path) else {
        eprintln!("warn: frame {frame_idx} 缺 GT，跳过 PSNR");
        return Ok(None);
    };
    match image.psnr_db(&gt) {
        Some(db) => Ok(Some(db)),
        None => {
            eprintln!(
                "warn: frame {frame_idx} 渲染 {}×{} 与 GT {}×{} 尺寸不一致，跳过 PSNR",
                image.width, image.height, gt.width, gt.height
            );
            Ok(None)
        }
    }
}

fn depth_path(rgb: &Path) -> PathBuf {
    let stem = rgb.file_stem().and_then(|s| s.to_str()).unwrap_or("splat");
    let name = format!("{stem}.depth.png");
    match rgb.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

fn write_depth_png(
    path: &Path,
    width: u32,
    height: u32,
    depth: &[f32],
    depth_max: f32,
) -> Result<(), String> {
    if depth.len() != (width as usize) * (height as usize) {
        return Err(format!(
            "深度长度 {} 与 {}×{} 不一致",
            depth.len(),
            width,
            height
        ));
    }
    let scale = 65535.0 / depth_max;
    let mut clipped = 0usize;
    let mut data = Vec::with_capacity(depth.len() * 2);
    for &d in depth {
        let q = if d.is_finite() {
            if d > depth_max {
                clipped += 1;
            }
            (d * scale).clamp(0.0, 65535.0).round() as u16
        } else {
            u16::MAX
        };
        data.extend_from_slice(&q.to_be_bytes());
    }
    if clipped > 0 {
        eprintln!(
            "warn: {} 像素深度超过 --depth-max {}，已饱和到 65535",
            clipped, depth_max
        );
    }
    let file = std::fs::File::create(path).map_err(|e| format!("建深度 PNG 失败: {e}"))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Sixteen);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&data).map_err(|e| e.to_string())
}
