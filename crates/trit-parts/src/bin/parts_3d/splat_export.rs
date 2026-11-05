//! `splat-export`：把多视角高斯拟合成**标准 3DGS PLY**，并写复核清单。
//!
//! ```text
//! parts-3d splat-export --scene <dir> --frames 0,2,4,6 --eval 1,3,5,7 \
//!   --cell 0.01 --scale-k 0.6 --alpha 0.8 --fit 30 [--fit-all] --out <ply>
//! ```
//!
//! 位置/颜色/尺度/alpha 是连续量，走 f32 与 CPU 可微光栅化（§6.7/6.8）；
//! 导出前对高斯做全序排序（`build` 走 HashMap，顺序随机）——否则同输入两次运行
//! 会得到不同字节，复核也无法逐字节比对。

use std::path::PathBuf;

use trit_parts::data::{read_npy_i32, read_rgb};
use trit_parts::fuse::View;
use trit_parts::geo::{Camera, read_npy_f32};
use trit_parts::splat::{build, fit_all, fit_colors, masked_psnr, sort_splats, write_ply_3dgs};

use super::cli::{arg, fail, frame_path, usage};
use super::look::avg;

fn parse_list(argv: &[String], k: &str, d: Vec<usize>) -> Vec<usize> {
    arg(argv, k)
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or(d)
}

fn load_view(scene: &std::path::Path, f: usize) -> View {
    let (ids, shape) =
        read_npy_i32(&frame_path(scene, "seg", f, "npy")).unwrap_or_else(|e| fail(&e));
    let (d, _) = read_npy_f32(&frame_path(scene, "depth", f, "npy")).unwrap_or_else(|e| fail(&e));
    View {
        ids: ids.iter().map(|&v| v.max(0) as u32).collect(),
        depth: d,
        w: shape[1],
        h: shape[0],
        cam: Camera::load(&frame_path(scene, "meta", f, "json")).unwrap_or_else(|e| fail(&e)),
    }
}

fn load_rgb(scene: &std::path::Path, f: usize) -> Vec<f32> {
    read_rgb(&frame_path(scene, "rgb", f, "png"))
        .map(|(_, _, p)| p)
        .unwrap_or_else(|e| fail(&e))
}

fn mean_psnr(splats: &[trit_parts::splat::Splat], views: &[View], rgbs: &[Vec<f32>]) -> f64 {
    let ps: Vec<f64> = views
        .iter()
        .zip(rgbs)
        .map(|(v, gt)| {
            let img = trit_parts::splat::render(splats, &v.cam, v.w, v.h, [0.0, 0.0, 0.0]);
            masked_psnr(&img, gt, &v.ids)
        })
        .collect();
    avg(&ps)
}

fn num(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.6}")
    } else {
        "null".into()
    }
}

pub(crate) fn splat_export_cmd(argv: &[String]) {
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let fit_frames = parse_list(argv, "--frames", vec![0, 2, 4, 6]);
    let eval_frames = parse_list(argv, "--eval", vec![1, 3, 5, 7]);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let scale_k: f32 = arg(argv, "--scale-k")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.6);
    let alpha: f32 = arg(argv, "--alpha")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.8);
    let iters: usize = arg(argv, "--fit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let fit_all_mode = argv.iter().any(|a| a == "--fit-all");
    let lr_color: f32 = arg(argv, "--fit-lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5);
    let lr_pos: f32 = arg(argv, "--fit-pos")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.02);
    let lr_scale: f32 = arg(argv, "--fit-scale")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1e-4);
    let lr_alpha: f32 = arg(argv, "--fit-alpha")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let reg: f32 = arg(argv, "--fit-reg")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0);
    if fit_frames.is_empty() || eval_frames.is_empty() {
        fail("--frames / --eval 不能为空");
    }
    let mut all: Vec<usize> = fit_frames.iter().chain(&eval_frames).copied().collect();
    all.sort_unstable();
    all.dedup();

    let fit_views: Vec<View> = fit_frames.iter().map(|&f| load_view(&scene, f)).collect();
    let fit_rgbs: Vec<Vec<f32>> = fit_frames.iter().map(|&f| load_rgb(&scene, f)).collect();
    let eval_views: Vec<View> = eval_frames.iter().map(|&f| load_view(&scene, f)).collect();
    let eval_rgbs: Vec<Vec<f32>> = eval_frames.iter().map(|&f| load_rgb(&scene, f)).collect();

    let mut splats = build(&fit_views, &fit_rgbs, cell, scale_k);
    for s in splats.iter_mut() {
        // 与 PLY 写入用同一夹紧范围，否则重渲与导出时的 PSNR 会差一点
        s.alpha = s.alpha.clamp(1e-4, 1.0 - 1e-4);
    }
    sort_splats(&mut splats);
    let fit0 = mean_psnr(&splats, &fit_views, &fit_rgbs);
    let eval0 = mean_psnr(&splats, &eval_views, &eval_rgbs);

    let sets: Vec<trit_parts::splat::FitView> = fit_views
        .iter()
        .zip(&fit_rgbs)
        .map(|(v, rgb)| (v.cam.clone(), v.w, v.h, rgb.clone(), v.ids.clone()))
        .collect();
    if iters > 0 {
        if fit_all_mode {
            fit_all(
                &mut splats,
                &sets,
                trit_parts::splat::FitOpts {
                    iters,
                    lr_color,
                    lr_pos,
                    lr_scale,
                    lr_alpha,
                    reg,
                },
                [0.0, 0.0, 0.0],
            );
        } else {
            fit_colors(&mut splats, &sets, iters, lr_color, [0.0, 0.0, 0.0]);
        }
    }
    let fit1 = mean_psnr(&splats, &fit_views, &fit_rgbs);
    let eval1 = mean_psnr(&splats, &eval_views, &eval_rgbs);

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).unwrap_or_else(|e| fail(&format!("{}: {e}", dir.display())));
    }
    write_ply_3dgs(&out, &splats).unwrap_or_else(|e| fail(&e));
    let sha = trit_core::fingerprint::file_sha256(&out).unwrap_or_default();
    let bytes = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let name = out
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("splat.ply")
        .to_string();
    println!(
        "高斯 {} 个 → {}（{} B，sha {}）",
        splats.len(),
        out.display(),
        bytes,
        &sha[..sha.len().min(12)]
    );
    println!("拟合视角 PSNR {fit0:.2} → {fit1:.2}；留出视角 {eval0:.2} → {eval1:.2}",);

    let fp = trit_parts::scenefp::fingerprint_scene_frames_rgb(&scene, &all)
        .unwrap_or_else(|e| fail(&e));
    let commit = trit_parts::partsrun::git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !trit_parts::partsrun::git_out(&["status", "--porcelain"])
        .trim()
        .is_empty();
    let argv_json: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| trit_parts::partsrun::json_str(&a))
        .collect();
    let frames_json = |v: &[usize]| {
        v.iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-splat\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {{\"fit_frames\": [{}], \"eval_frames\": [{}], \"cell\": {cell}, \"scale_k\": {scale_k}, \"alpha\": {alpha}, \"fit_iters\": {iters}, \"fit_all\": {fit_all_mode}, \"lr_color\": {lr_color}, \"lr_pos\": {lr_pos}, \"lr_scale\": {lr_scale}, \"lr_alpha\": {lr_alpha}, \"reg\": {reg}}},\n  \"data\": {{\"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"scene\": {}}},\n  \"splat\": {{\"path\": {}, \"sha256\": \"{sha}\", \"bytes\": {bytes}, \"count\": {}}},\n  \"metrics\": {{\"fit_psnr_before\": {}, \"fit_psnr_after\": {}, \"eval_psnr_before\": {}, \"eval_psnr_after\": {}}}\n}}\n",
        trit_parts::partsrun::created_unix(),
        trit_parts::partsrun::json_str(&commit),
        argv_json.join(", "),
        frames_json(&fit_frames),
        frames_json(&eval_frames),
        fp.files,
        fp.bytes,
        fp.hash,
        trit_parts::partsrun::json_str(&trit_parts::scenefp::norm_root(&scene)),
        trit_parts::partsrun::json_str(&name),
        splats.len(),
        num(fit0),
        num(fit1),
        num(eval0),
        num(eval1),
    );
    let mut mp = out.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mp = PathBuf::from(mp);
    std::fs::write(&mp, body).unwrap_or_else(|e| fail(&format!("{}: {e}", mp.display())));
    println!("清单 {}", mp.display());
}
