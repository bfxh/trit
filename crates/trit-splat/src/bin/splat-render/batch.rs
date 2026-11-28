//! 批量渲染编排：资产只读一次；支持并行帧、PSNR 门与输出端色彩处理。
use crate::cli;
use crate::mip;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use trit_splat::camera::parse_transforms;
#[path = "batch/color_args.rs"]
mod color_args;
#[path = "batch/frames.rs"]
mod frames;
#[path = "batch/load.rs"]
mod load;
#[path = "batch/metrics.rs"]
mod metrics;
#[path = "batch/parallel.rs"]
mod parallel;
use frames::{frame_paths, parse_frames, parse_threads};
use load::load_gaussians;
use metrics::{db_label, mean_psnr, write_psnr_json};
const USAGE: &str = "用法: splat-render --dataset <root> --frame N|all|a..b|csv --ply <ply|spz> --out <png|dir|模板{frame}> [--threads N|auto] [--depth] [--depth-max M] [--color-in raw|srgb|bt1886] [--color-out raw|srgb|bt1886] [--gamut srgb|p3|rec2020] [--exposure EV] [--temperature K] [--tint T] [--saturation S] [--tone-map none|reinhard|aces] [--bg auto|r,g,b] [--sh-degree 0..3] [--min-scale-factor F] [--psnr] [--psnr-json <path>] [--fail-under DB]";
pub fn run(argv: Vec<String>) -> ExitCode {
    match run_inner(&argv) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_inner(argv: &[String]) -> Result<ExitCode, String> {
    let Some(dataset) = cli::value(argv, "--dataset") else {
        eprintln!("{USAGE}");
        return Ok(ExitCode::from(2));
    };
    let ply_path = cli::value(argv, "--ply").unwrap_or_else(|| "scene_30000.ply".into());
    let out = cli::value(argv, "--out").unwrap_or_else(|| "splat.png".into());
    let bg_mode = cli::value(argv, "--bg").unwrap_or_else(|| "auto".into());
    let sh_degree: usize = cli::value(argv, "--sh-degree")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let min_scale_factor: f32 = cli::value(argv, "--min-scale-factor")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    let frame_spec = cli::value(argv, "--frames")
        .or_else(|| cli::value(argv, "--frame"))
        .unwrap_or_else(|| "0".into());
    let threads = parse_threads(cli::value(argv, "--threads").as_deref())?;
    let psnr_json = cli::value(argv, "--psnr-json");
    let fail_under = cli::value(argv, "--fail-under")
        .map(|v| {
            v.parse::<f64>()
                .map_err(|_| format!("--fail-under 非法: {v}"))
        })
        .transpose()?;
    let want_psnr = cli::flag(argv, "--psnr") || psnr_json.is_some() || fail_under.is_some();
    let want_depth = cli::flag(argv, "--depth");
    let depth_max: f32 = cli::value(argv, "--depth-max")
        .map(|v| {
            v.parse::<f32>()
                .map_err(|_| format!("--depth-max 非法: {v}"))
        })
        .transpose()?
        .unwrap_or(100.0);
    if want_depth && !(depth_max > 0.0 && depth_max.is_finite()) {
        return Err(format!("--depth-max 必须为正有限值: {depth_max}"));
    }
    let color = color_args::parse(argv)?;

    let transforms_path = PathBuf::from(&dataset).join("transforms.json");
    let text = std::fs::read_to_string(&transforms_path)
        .map_err(|e| format!("读 transforms 失败: {e}"))?;
    let transforms = parse_transforms(&text).map_err(|e| format!("transforms.json: {e}"))?;
    let frames = parse_frames(&frame_spec, transforms.frames.len())?;

    let mut gaussians = load_gaussians(&ply_path)?;
    println!("ply: {} 高斯 (sh-degree={sh_degree})", gaussians.len());
    cli::truncate_sh(&mut gaussians, sh_degree);
    let min_scales = mip::min_scales(&gaussians, &transforms, min_scale_factor);
    let paths = frame_paths(&out, &frames)?;

    let psnrs = parallel::render_frames(
        &gaussians,
        &transforms,
        min_scales.as_deref(),
        &dataset,
        &bg_mode,
        want_psnr,
        want_depth,
        depth_max,
        &color,
        &frames,
        &paths,
        threads,
    )?;

    let mut metrics: Vec<(usize, Option<f64>)> = Vec::new();
    for (slot, &frame_idx) in frames.iter().enumerate() {
        let cam = &transforms.frames[frame_idx];
        println!(
            "frame {frame_idx}: {} 相机位置 ({:.2},{:.2},{:.2}) -> {:?}",
            cam.file_path,
            cam.c2w.translation().x,
            cam.c2w.translation().y,
            cam.c2w.translation().z,
            paths[slot]
        );
        if let Some(db) = psnrs[slot] {
            println!("  PSNR {}: {}", frame_idx, db_label(db));
            metrics.push((frame_idx, Some(db)));
        }
    }

    let mean = mean_psnr(&metrics);
    if want_psnr {
        println!(
            "PSNR: {} 帧，均值 {}",
            metrics.len(),
            mean.map(db_label).unwrap_or_else(|| "n/a".into())
        );
    }
    if let Some(path) = psnr_json {
        write_psnr_json(Path::new(&path), &metrics, mean)?;
    }
    if let Some(threshold) = fail_under {
        match mean {
            Some(m) if m.is_finite() && m < threshold => {
                eprintln!("PSNR 均值 {m:.2} dB < 阈值 {threshold:.2} dB");
                return Ok(ExitCode::from(3));
            }
            Some(m) if m.is_infinite() => {}
            _ => eprintln!("warn: 无有效 PSNR，--fail-under 不判"),
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
#[path = "batch/tests.rs"]
mod tests;
