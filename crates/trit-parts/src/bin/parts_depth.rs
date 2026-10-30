//! `parts-depth`：冻结骨干 + f32 线性深度头（视差）的训练与评估。
//!
//! - `train --model <tpm> --data <dir,...> --steps N --lr 0.05 --save head.tpdh`
//! - `eval --model <tpm> --head head.tpdh --scene <dir> [--limit N] [--3d --manifest parts.json]`

use std::path::{Path, PathBuf};

use trit_parts::data::read_rgb;
use trit_parts::depth::{CH, DepthHead, to_disparity};
use trit_parts::gpu::Gpu;
use trit_parts::net::Net;
use trit_parts::tpm::TpmModel;

fn arg(argv: &[String], key: &str) -> Option<String> {
    let i = argv.iter().position(|a| a == key)?;
    argv.get(i + 1).cloned()
}

fn frames(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir.join("rgb"))
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn stem(p: &Path) -> String {
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn load_depth_valid(scene: &Path, stem: &str) -> (Vec<f32>, Vec<u8>) {
    let (dep, _) = trit_parts::geo::read_npy_f32(&scene.join("depth").join(format!("{stem}.npy")))
        .unwrap_or_else(|e| panic!("{e}"));
    let (seg, _) = trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{stem}.npy")))
        .unwrap_or_else(|e| panic!("{e}"));
    let valid: Vec<u8> = seg.iter().map(|&v| u8::from(v > 0)).collect();
    (to_disparity(&dep), valid)
}

fn forward_all(net: &Net, gpu: &Gpu, p: &Path) -> (Vec<f32>, Vec<f32>, usize) {
    let (w, _h, rgb) = read_rgb(p).unwrap_or_else(|e| panic!("{e}"));
    let xb = gpu.buf(&rgb, "x");
    let tr = net.forward(gpu, xb, w as u32);
    let npix = w * w;
    let feats = gpu.read(&tr.z, npix * CH);
    let logits = gpu.read(&tr.out, npix * 16);
    (feats, logits, npix)
}

fn spread(all: &[(PathBuf, PathBuf)], k: usize) -> Vec<(PathBuf, PathBuf)> {
    let step = (all.len() / k.max(1)).max(1);
    all.iter().step_by(step).take(k).cloned().collect()
}

fn train(argv: &[String]) {
    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let data: Vec<PathBuf> = arg(argv, "--data")
        .unwrap_or_else(|| usage())
        .split(',')
        .map(PathBuf::from)
        .collect();
    let steps: usize = arg(argv, "--steps")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let calib: usize = arg(argv, "--calib")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let save = PathBuf::from(arg(argv, "--save").unwrap_or_else(|| usage()));
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);

    let gpu = Gpu::new().expect("GPU 初始化失败");
    let tpm = TpmModel::load(&model).unwrap_or_else(|e| panic!("{e}"));
    let mut net = Net::from_tpm(&tpm);
    net.upload_all(&gpu);

    let mut all: Vec<(PathBuf, PathBuf)> = Vec::new();
    for d in &data {
        for p in frames(d) {
            all.push((d.clone(), p));
        }
    }
    if all.is_empty() {
        panic!("没有可用帧");
    }
    println!("帧 {}，lr {lr}，calib {calib}", all.len());

    let mut head = DepthHead::zeros(CH);
    let mut acc = vec![0f64; CH];
    let mut acc2 = vec![0f64; CH];
    let mut n_pix = 0usize;
    let mut gt_sum = 0f64;
    let mut gt_n = 0usize;
    for (scene, p) in spread(&all, calib).iter() {
        let (feats, _, npix) = forward_all(&net, &gpu, p);
        for i in 0..npix {
            for c in 0..CH {
                let v = f64::from(feats[i * CH + c]);
                acc[c] += v;
                acc2[c] += v * v;
            }
        }
        n_pix += npix;
        let (gt, valid) = load_depth_valid(scene, &stem(p));
        for i in 0..npix {
            if valid[i] > 0 {
                gt_sum += f64::from(gt[i]);
                gt_n += 1;
            }
        }
    }
    for c in 0..CH {
        let m = acc[c] / n_pix as f64;
        let var = (acc2[c] / n_pix as f64 - m * m).max(1e-12);
        head.mu[c] = m as f32;
        head.sd[c] = (var.sqrt() as f32).max(1e-3);
    }
    head.b = (gt_sum / gt_n.max(1) as f64) as f32;
    println!("bias 初值（视差均值）{:.4}", head.b);

    let fit_frames: usize = arg(argv, "--fit-frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(40);
    let stride: usize = arg(argv, "--stride")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let ridge: f64 = arg(argv, "--ridge")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1e-2);
    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);

    let pick = spread(&all, fit_frames);
    let mut xs: Vec<f32> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for (scene, p) in pick.iter() {
        let (feats, _, npix) = forward_all(&net, &gpu, p);
        let (gt, valid) = load_depth_valid(scene, &stem(p));
        for i in (0..npix).step_by(stride.max(1)) {
            if valid[i] == 0 {
                continue;
            }
            for c in 0..CH {
                let v = (feats[i * CH + c] - head.mu[c]) / head.sd[c];
                xs.push(v.clamp(-8.0, 8.0));
            }
            ys.push(gt[i]);
        }
    }
    let n = ys.len();
    let (w, b) = DepthHead::fit_irls(&xs, n, &ys, ridge, iters);
    head.w = w;
    head.b = b;
    let mut l1 = 0f64;
    for i in 0..n {
        let mut acc = f64::from(head.b);
        for c in 0..CH {
            acc += f64::from(head.w[c]) * f64::from(xs[i * CH + c]);
        }
        l1 += (acc - f64::from(ys[i])).abs();
    }
    println!(
        "IRLS 拟合：{} 帧 / {} 像素（跨 {} 个距离目录），训练视差 L1 {:.4}",
        pick.len(),
        n,
        data.len(),
        l1 / n.max(1) as f64
    );
    let _ = (steps, lr, seed);
    head.save(&save).unwrap_or_else(|e| panic!("{e}"));
    println!("已存 {}", save.display());
}

fn eval(argv: &[String]) {
    use trit_parts::api::decode_ids;
    use trit_parts::geo::{Camera, centroid, clouds, iou3d, voxelize};
    use trit_parts::parts::PartList;

    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let head_p = PathBuf::from(arg(argv, "--head").unwrap_or_else(|| usage()));
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let limit: usize = arg(argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(usize::MAX);
    let three_d = argv.iter().any(|a| a == "--3d");
    let voxel: f32 = arg(argv, "--voxel")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let min_vox: usize = arg(argv, "--min-vox")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let manifest = arg(argv, "--manifest").map(PathBuf::from);

    let gpu = Gpu::new().expect("GPU 初始化失败");
    let tpm = TpmModel::load(&model).unwrap_or_else(|e| panic!("{e}"));
    let mut net = Net::from_tpm(&tpm);
    net.upload_all(&gpu);
    let head = DepthHead::load(&head_p).unwrap_or_else(|e| panic!("{e}"));
    let parts = manifest
        .as_ref()
        .map(|m| PartList::load(m).unwrap_or_else(|e| panic!("{e}")));

    let mut abs_e = 0f64;
    let mut rel_e = 0f64;
    let mut delta = 0f64;
    let mut n = 0usize;
    let mut ious: Vec<f32> = Vec::new();
    let mut frames_n = 0usize;
    for p in frames(&scene).into_iter().take(limit) {
        let s = stem(&p);
        let (feats, logits, npix) = forward_all(&net, &gpu, &p);
        let res = npix.isqrt() as u32;
        let disp_all = head.disparity(&feats, npix);
        let pred_depth = DepthHead::depth(&disp_all);
        let (gt_disp, valid) = load_depth_valid(&scene, &s);
        if std::env::var("DEBUG_DEPTH").is_ok() && frames_n == 0 {
            let mut shown = 0;
            for i in 0..npix {
                if valid[i] > 0 && shown < 5 {
                    println!(
                        "px {i}: pred disp {:.3} -> {:.2} m, gt {:.2} m",
                        disp_all[i],
                        pred_depth[i],
                        1.0 / gt_disp[i]
                    );
                    shown += 1;
                }
            }
        }
        for i in 0..npix {
            if valid[i] == 0 {
                continue;
            }
            let g = 1.0 / gt_disp[i];
            let d = f64::from(pred_depth[i]);
            let e = (d - f64::from(g)).abs();
            abs_e += e;
            rel_e += e / f64::from(g);
            delta += f64::from(u8::from(d.max(f64::from(g)) / d.min(f64::from(g)) < 1.25));
            n += 1;
        }
        if three_d && let Some(pl) = &parts {
            let pred_ids = decode_ids(&logits, res, pl);
            let (gt_i32, shape) =
                trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{s}.npy")))
                    .unwrap_or_else(|e| panic!("{e}"));
            let (gt_dep, _) =
                trit_parts::geo::read_npy_f32(&scene.join("depth").join(format!("{s}.npy")))
                    .unwrap_or_else(|e| panic!("{e}"));
            let (h, w) = (shape[0], shape[1]);
            let cam = Camera::load(&scene.join("meta").join(format!("{s}.json")))
                .unwrap_or_else(|e| panic!("{e}"));
            let gt_ids: Vec<u32> = gt_i32.iter().map(|&v| v.max(0) as u32).collect();
            let gt_parts = clouds(&gt_ids, &gt_dep, &cam, w, h);
            let pred_parts = clouds(&pred_ids, &pred_depth, &cam, w, h);
            for (id, gpts) in &gt_parts {
                let gvox = voxelize(gpts, voxel);
                if gvox.len() < min_vox {
                    continue;
                }
                let pvox = pred_parts
                    .get(id)
                    .map(|q| voxelize(q, voxel))
                    .unwrap_or_default();
                let i = iou3d(&gvox, &pvox);
                if i.is_finite() {
                    ious.push(i);
                }
                let _ = centroid(gpts);
            }
        }
        frames_n += 1;
    }
    println!(
        "帧 {frames_n}，像素 {n}：深度 MAE {:.1} mm，相对误差 {:.2}%，δ<1.25 {:.3}",
        abs_e / n as f64 * 1000.0,
        rel_e / n as f64 * 100.0,
        delta / n as f64
    );
    if !ious.is_empty() {
        let mean = ious.iter().sum::<f32>() / ious.len() as f32;
        println!("3D IoU（预测深度）均值 {mean:.4}，样本 {}", ious.len());
    }
}

fn usage() -> ! {
    eprintln!(
        "用法:
  parts-depth train --model <tpm> --data <dir,...> --steps N --lr 0.05 --save <tph>
  parts-depth eval --model <tpm> --head <tph> --scene <dir> [--limit N]"
    );
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("train") => train(&argv),
        Some("eval") => eval(&argv),
        _ => usage(),
    }
}
