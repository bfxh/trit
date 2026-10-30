//! `parts-3d`：部件 id 图 + 深度 → 3D 点云（PLY），以及逐部件 3D IoU 评估。
//!
//! 三个子命令：
//! - `cloud`：GT seg + depth → 逐部件着色的 `.ply`；
//! - `infer`：模型预测 id + **外部深度**（任意来源，默认取 scene/depth）→ `.ply`（生产路径）；
//! - `eval`：模型预测 id + GT depth → 与 GT 反投影逐部件比对（3D IoU / 质心误差）。

use std::path::{Path, PathBuf};

use trit_parts::data::read_npy_i32;
use trit_parts::geo::{Camera, centroid, clouds, iou3d, read_npy_f32, voxelize, write_ply};

fn arg(argv: &[String], key: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == key)?;
    argv.get(i + 1).cloned()
}

fn frame_path(dir: &Path, sub: &str, n: usize, ext: &str) -> PathBuf {
    dir.join(sub).join(format!("{n:06}.{ext}"))
}

fn cloud(argv: &[String]) {
    let seg = PathBuf::from(arg(argv, "--seg").unwrap_or_else(|| usage()));
    let depth = PathBuf::from(arg(argv, "--depth").unwrap_or_else(|| usage()));
    let meta = PathBuf::from(arg(argv, "--meta").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));

    let cam = Camera::load(&meta).unwrap_or_else(|e| fail(&e));
    let (ids, shape) = read_npy_i32(&seg).unwrap_or_else(|e| fail(&e));
    let (dep, dshape) = read_npy_f32(&depth).unwrap_or_else(|e| fail(&e));
    if shape != dshape || shape.len() != 2 {
        fail("seg 与 depth 形状不一致");
    }
    let (h, w) = (shape[0], shape[1]);
    let ids: Vec<u32> = ids.iter().map(|&v| v.max(0) as u32).collect();
    let parts = clouds(&ids, &dep, &cam, w, h);
    write_ply(&out, &parts).unwrap_or_else(|e| fail(&format!("写 PLY 失败: {e}")));
    let n: usize = parts.values().map(Vec::len).sum();
    println!("部件 {} 个，点 {} 个 → {}", parts.len(), n, out.display());
}

/// `infer --model <tpm> --scene <dir> --frame N --manifest <json> --out <ply> [--depth <npy>]`
/// 模型出部件 id，深度由外部提供（本仓不预测深度），反投影后写逐部件 PLY。
#[cfg(feature = "gpu")]
fn infer(argv: &[String]) {
    use trit_parts::api::{infer_ids, wgpu_from_tpm};
    use trit_parts::data::read_rgb;
    use trit_parts::parts::PartList;
    use trit_parts::tpm::TpmModel;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let frame: usize = arg(argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let depth_path = arg(argv, "--depth").map(PathBuf::from);

    let tpm = TpmModel::load(&model).unwrap_or_else(|e| fail(&e));
    let parts = PartList::load(&manifest).unwrap_or_else(|e| fail(&e));
    let mut backend = wgpu_from_tpm(&tpm).unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));

    let rgb_p = frame_path(&scene, "rgb", frame, "png");
    let (w, _h, rgb) = read_rgb(&rgb_p).unwrap_or_else(|e| fail(&e));
    let ids = match infer_ids(&mut backend, &rgb, w as u32, &parts) {
        Ok(v) => v,
        Err(e) => fail(&format!("推理失败: {e}")),
    };
    let depth = depth_path.unwrap_or_else(|| frame_path(&scene, "depth", frame, "npy"));
    let (dep, shape) = read_npy_f32(&depth).unwrap_or_else(|e| fail(&e));
    if shape.len() != 2 || shape[0] * shape[1] != ids.len() {
        fail("深度与 id 图尺寸不一致");
    }
    let cam = Camera::load(&frame_path(&scene, "meta", frame, "json")).unwrap_or_else(|e| fail(&e));
    let parts3 = clouds(&ids, &dep, &cam, shape[1], shape[0]);
    write_ply(&out, &parts3).unwrap_or_else(|e| fail(&format!("写 PLY 失败: {e}")));
    let n: usize = parts3.values().map(Vec::len).sum();
    println!(
        "帧 {frame}：部件 {} 个，点 {} 个 → {}",
        parts3.len(),
        n,
        out.display()
    );
}

#[cfg(feature = "gpu")]
fn eval(argv: &[String]) {
    use trit_parts::api::{infer_ids, wgpu_from_tpm};
    use trit_parts::data::read_rgb;
    use trit_parts::parts::PartList;
    use trit_parts::tpm::TpmModel;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let limit: usize = arg(argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let voxel: f32 = arg(argv, "--voxel")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let min_vox: usize = arg(argv, "--min-vox")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    // 自检：用 GT 当预测（度量应给 1.0）。度量本身不准时这里会先红。
    let gt_as_pred = argv.iter().any(|a| a == "--gt");

    let tpm = TpmModel::load(&model).unwrap_or_else(|e| fail(&e));
    let parts = PartList::load(&manifest).unwrap_or_else(|e| fail(&e));
    let mut backend = wgpu_from_tpm(&tpm).unwrap_or_else(|e| panic!("wgpu 后端不可用: {e}"));

    let mut ious: Vec<f32> = Vec::new();
    let mut cdist: Vec<f32> = Vec::new();
    let mut frames = 0usize;
    for n in 0..limit {
        let rgb_p = frame_path(&scene, "rgb", n, "png");
        if !rgb_p.exists() {
            break;
        }
        let (w, _h, rgb) = read_rgb(&rgb_p).unwrap_or_else(|e| fail(&e));
        let (gt, shape) =
            read_npy_i32(&frame_path(&scene, "seg", n, "npy")).unwrap_or_else(|e| fail(&e));
        let (dep, ds) =
            read_npy_f32(&frame_path(&scene, "depth", n, "npy")).unwrap_or_else(|e| fail(&e));
        if shape != ds {
            fail("seg 与 depth 形状不一致");
        }
        let (h, sw) = (shape[0], shape[1]);
        let cam = Camera::load(&frame_path(&scene, "meta", n, "json")).unwrap_or_else(|e| fail(&e));
        let gt_ids: Vec<u32> = gt.iter().map(|&v| v.max(0) as u32).collect();
        let pred = if gt_as_pred {
            gt_ids.clone()
        } else {
            match infer_ids(&mut backend, &rgb, w as u32, &parts) {
                Ok(v) => v,
                Err(e) => fail(&format!("推理失败: {e}")),
            }
        };
        let gt_parts = clouds(&gt_ids, &dep, &cam, sw, h);
        let pred_parts = clouds(&pred, &dep, &cam, sw, h);

        for (id, gpts) in &gt_parts {
            let gvox = voxelize(gpts, voxel);
            if gvox.len() < min_vox {
                continue;
            }
            let pvox = pred_parts
                .get(id)
                .map(|p| voxelize(p, voxel))
                .unwrap_or_default();
            let i = iou3d(&gvox, &pvox);
            if i.is_finite() {
                ious.push(i);
            }
            if let Some(ppts) = pred_parts.get(id) {
                let (a, b) = (centroid(gpts), centroid(ppts));
                cdist.push(
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt(),
                );
            }
        }
        frames += 1;
    }

    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let mut sorted = ious.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = if sorted.is_empty() {
        f32::NAN
    } else {
        sorted[sorted.len() / 2]
    };
    println!(
        "帧 {frames}，部件样本 {}，体素 {:.0} mm，GT≥{min_vox} 体素",
        ious.len(),
        voxel * 1000.0
    );
    println!(
        "逐部件 3D IoU 均值 {:.4} / 中位 {:.4}；质心误差 均值 {:.1} mm",
        mean(&ious),
        med,
        mean(&cdist) * 1000.0
    );
    if gt_as_pred {
        let ok = (mean(&ious) - 1.0).abs() < 1e-4 && mean(&cdist) < 1e-6;
        println!("自检 --gt：{}", if ok { "通过（IoU=1.0）" } else { "失败" });
        if !ok {
            fail("自检失败：GT 对 GT 的 3D 度量不是恒等，度量有 bug");
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "用法:
  parts-3d cloud --seg <npy> --depth <npy> --meta <json> --out <ply>
  parts-3d infer --model <tpm> --scene <dir> --frame N --manifest <json> --out <ply> [--depth <npy>]
  parts-3d eval --model <tpm> --scene <dir> --manifest <json> [--limit N] [--voxel 0.01] [--min-vox 30]"
    );
    std::process::exit(2);
}

fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("cloud") => cloud(&argv),
        Some("infer") => {
            #[cfg(feature = "gpu")]
            infer(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        Some("eval") => {
            #[cfg(feature = "gpu")]
            eval(&argv);
            #[cfg(not(feature = "gpu"))]
            fail("本构建未启用 gpu feature");
        }
        _ => usage(),
    }
}
