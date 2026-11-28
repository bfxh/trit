//! 批量渲染编排：资产只读一次，逐帧复用；可选 PSNR 报表与阈值门。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trit_splat::camera::parse_transforms;
use trit_splat::ply::{RawGaussian, parse_ply};
use trit_splat::raster::render;

use crate::cli;
use crate::mip;

const USAGE: &str = "用法: splat-render --dataset <root> --frame N|all|a..b|csv --ply <ply|spz> --out <png|dir|模板{frame}> [--bg auto|r,g,b] [--sh-degree 0..3] [--min-scale-factor F] [--psnr] [--psnr-json <path>] [--fail-under DB]";

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
    let psnr_json = cli::value(argv, "--psnr-json");
    let fail_under = cli::value(argv, "--fail-under")
        .map(|v| {
            v.parse::<f64>()
                .map_err(|_| format!("--fail-under 非法: {v}"))
        })
        .transpose()?;
    let want_psnr = cli::flag(argv, "--psnr") || psnr_json.is_some() || fail_under.is_some();

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
    let mut metrics: Vec<(usize, Option<f64>)> = Vec::new();
    for (slot, &frame_idx) in frames.iter().enumerate() {
        let cam = &transforms.frames[frame_idx];
        let background = cli::background(&bg_mode, &dataset, &cam.file_path);
        let image = render(
            &gaussians,
            &cam.c2w,
            &transforms.intrinsics,
            background,
            min_scales.as_deref(),
        );
        cli::write_png(&paths[slot], &image).map_err(|e| format!("写 PNG: {e}"))?;
        println!(
            "frame {frame_idx}: {} 相机位置 ({:.2},{:.2},{:.2}) -> {:?}",
            cam.file_path,
            cam.c2w.translation().x,
            cam.c2w.translation().y,
            cam.c2w.translation().z,
            paths[slot]
        );
        if !want_psnr {
            continue;
        }
        let gt_path = Path::new(&dataset).join(&cam.file_path);
        match cli::decode_gt_rgb(&gt_path) {
            Some(gt) => match image.psnr_db(&gt) {
                Some(db) => {
                    println!("  PSNR {}: {}", frame_idx, db_label(db));
                    metrics.push((frame_idx, Some(db)));
                }
                None => eprintln!(
                    "warn: frame {frame_idx} 渲染 {}×{} 与 GT {}×{} 尺寸不一致，跳过 PSNR",
                    image.width, image.height, gt.width, gt.height
                ),
            },
            None => eprintln!("warn: frame {frame_idx} 缺 GT，跳过 PSNR"),
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
        write_psnr_json(&path, &metrics, mean)?;
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

fn load_gaussians(path: &str) -> Result<Vec<RawGaussian>, String> {
    if path.to_ascii_lowercase().ends_with(".spz") {
        #[cfg(feature = "spz")]
        {
            return trit_splat::spz::load_spz(Path::new(path))
                .map_err(|e| format!("SPZ 解析: {e}"));
        }
        #[cfg(not(feature = "spz"))]
        {
            return Err("读 .spz 需要构建时启用 spz feature".into());
        }
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读 ply 失败 {path}: {e}"))?;
    parse_ply(&bytes).map_err(|e| format!("PLY 解析: {e}"))
}

/// 帧选择：`all`、半开区间 `a..b`、逗号列表；`--frame N` 是单帧别名。
fn parse_frames(spec: &str, total: usize) -> Result<Vec<usize>, String> {
    if total == 0 {
        return Err("数据集没有帧".into());
    }
    let mut out = Vec::new();
    for part in spec.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p.eq_ignore_ascii_case("all") {
            out.extend(0..total);
            continue;
        }
        if let Some((a, b)) = p.split_once("..") {
            let start = if a.trim().is_empty() {
                0
            } else {
                index(a.trim())?
            };
            let end = if b.trim().is_empty() {
                total
            } else {
                index(b.trim())?
            };
            if start >= end {
                return Err(format!("空帧区间: {p}"));
            }
            if end > total {
                return Err(format!("帧区间 {p} 超界（共 {total} 帧）"));
            }
            out.extend(start..end);
        } else {
            let i = index(p)?;
            if i >= total {
                return Err(format!("帧 {i} 超界（共 {total} 帧）"));
            }
            out.push(i);
        }
    }
    if out.is_empty() {
        return Err("没有选中任何帧".into());
    }
    let mut seen = HashSet::new();
    for &f in &out {
        if !seen.insert(f) {
            return Err(format!("帧 {f} 重复"));
        }
    }
    Ok(out)
}

fn index(s: &str) -> Result<usize, String> {
    s.parse::<usize>().map_err(|_| format!("非法帧号: {s}"))
}

/// 多帧输出：`{frame}` 模板，或把 `--out` 当目录；单帧保持原路径。
fn frame_paths(out: &str, frames: &[usize]) -> Result<Vec<PathBuf>, String> {
    if out.contains("{frame}") {
        return Ok(frames
            .iter()
            .map(|f| PathBuf::from(out.replace("{frame}", &format!("{f:04}"))))
            .collect());
    }
    if frames.len() == 1 {
        return Ok(vec![PathBuf::from(out)]);
    }
    if out.to_ascii_lowercase().ends_with(".png") {
        return Err("多帧输出请用目录，或在 --out 里放 {frame} 模板".into());
    }
    std::fs::create_dir_all(out).map_err(|e| format!("创建输出目录 {out}: {e}"))?;
    Ok(frames
        .iter()
        .map(|f| Path::new(out).join(format!("frame_{f:04}.png")))
        .collect())
}

fn mean_psnr(metrics: &[(usize, Option<f64>)]) -> Option<f64> {
    let values: Vec<f64> = metrics.iter().filter_map(|(_, db)| *db).collect();
    if values.is_empty() {
        return None;
    }
    if values.iter().any(|v| v.is_infinite()) {
        return Some(f64::INFINITY);
    }
    Some(values.iter().sum::<f64>() / values.len() as f64)
}

fn db_label(db: f64) -> String {
    if db.is_infinite() {
        "∞ dB".into()
    } else {
        format!("{db:.2} dB")
    }
}

fn write_psnr_json(
    path: &str,
    metrics: &[(usize, Option<f64>)],
    mean: Option<f64>,
) -> Result<(), String> {
    let rows: Vec<serde_json::Value> = metrics
        .iter()
        .map(|(frame, db)| {
            serde_json::json!({
                "frame": frame,
                "psnr_db": db.filter(|v| v.is_finite()),
                "infinite": db.map(|v| v.is_infinite()).unwrap_or(false),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "frames": rows,
        "mean_db": mean.filter(|v| v.is_finite()),
        "mean_infinite": mean.map(|v| v.is_infinite()).unwrap_or(false),
    });
    std::fs::write(
        path,
        serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写 PSNR JSON {path}: {e}"))
}

#[cfg(test)]
#[path = "batch/tests.rs"]
mod tests;
