//! 帧间并行：每帧独立渲染，资产与相机表只读共享。

use std::path::{Path, PathBuf};

use trit_splat::camera::Transforms;
use trit_splat::ply::RawGaussian;
use trit_splat::raster::render;

use crate::cli;

/// 渲染指定帧；`threads` 为 1 时串行，>1 时按连续块分给工作线程。
///
/// 返回与 `frames` 等长的 PSNR 数组（未算/缺 GT 为 None）。
#[allow(clippy::too_many_arguments)]
pub(super) fn render_frames(
    gaussians: &[RawGaussian],
    transforms: &Transforms,
    min_scales: Option<&[f32]>,
    dataset: &str,
    bg_mode: &str,
    want_psnr: bool,
    frames: &[usize],
    paths: &[PathBuf],
    threads: usize,
) -> Result<Vec<Option<f64>>, String> {
    if frames.len() != paths.len() {
        return Err(format!(
            "帧数 {} 与输出路径数 {} 不一致",
            frames.len(),
            paths.len()
        ));
    }
    let workers = threads.min(frames.len()).max(1);
    if workers <= 1 {
        let mut out = Vec::with_capacity(frames.len());
        for (&frame, path) in frames.iter().zip(paths) {
            out.push(render_slot(
                gaussians, transforms, min_scales, dataset, bg_mode, want_psnr, frame, path,
            )?);
        }
        return Ok(out);
    }

    let chunk = frames.len().div_ceil(workers);
    let mut results: Vec<Result<Vec<Option<f64>>, String>> = Vec::new();
    std::thread::scope(|scope| -> Result<(), String> {
        let mut handles = Vec::new();
        for (frame_chunk, path_chunk) in frames.chunks(chunk).zip(paths.chunks(chunk)) {
            handles.push(scope.spawn(move || {
                let mut out = Vec::with_capacity(frame_chunk.len());
                for (&frame, path) in frame_chunk.iter().zip(path_chunk) {
                    out.push(render_slot(
                        gaussians, transforms, min_scales, dataset, bg_mode, want_psnr, frame, path,
                    )?);
                }
                Ok::<_, String>(out)
            }));
        }
        for handle in handles {
            match handle.join() {
                Ok(result) => results.push(result),
                Err(_) => return Err("渲染线程 panic".into()),
            }
        }
        Ok(())
    })?;

    let mut out = Vec::with_capacity(frames.len());
    for result in results {
        out.extend(result?);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn render_slot(
    gaussians: &[RawGaussian],
    transforms: &Transforms,
    min_scales: Option<&[f32]>,
    dataset: &str,
    bg_mode: &str,
    want_psnr: bool,
    frame_idx: usize,
    path: &Path,
) -> Result<Option<f64>, String> {
    let cam = &transforms.frames[frame_idx];
    let background = cli::background(bg_mode, dataset, &cam.file_path);
    let image = render(
        gaussians,
        &cam.c2w,
        &transforms.intrinsics,
        background,
        min_scales,
    );
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
