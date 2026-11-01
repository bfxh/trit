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

/// `mesh-single --model <tpm> --head <tpdh> --scene <dir> --frame N --cell 0.02 [--gt] --out <ply>`：
/// **单图 → 3D 网格**：预测（或 GT）深度 → 反投影点云 → 体素填实 → 表面网 → PLY。
/// 前景遮罩目前取 GT seg（真实照片要靠背景分割/深度阈值，见文档）。
/// 一条训练样本：RGB 平面 / **原始 log 深度** / 有效掩码。
/// 归一化统计是**数据集级**的（存进 `DepthNet::med/iqr`）：逐图归一化会把绝对尺度抹掉，
/// 推理时拿不到「这张图自己的中位数」，网格就没有尺度。
type DepthSample = (Vec<Vec<f32>>, Vec<f32>, Vec<bool>);

/// 读一张图 + 深度，返回 `(rgb 平面, 原始 log 深度, 掩码)`。
///
/// `full=true` 时**整图盒式降采样**到 `size²`（前景只占 ~15%，中心裁剪会丢掉全局布局；
/// 64² 整图是 6×6 块均值）；否则中心裁剪。
fn load_sample(scene: &Path, frame: usize, size: usize, full: bool) -> Option<DepthSample> {
    let p = scene.join("rgb").join(format!("{frame:06}.png"));
    if !p.exists() {
        return None;
    }
    let (w, h, rgb) = read_rgb(&p).ok()?;
    let (dep, _) =
        trit_parts::geo::read_npy_f32(&scene.join("depth").join(format!("{frame:06}.npy"))).ok()?;
    let (seg, _) =
        trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{frame:06}.npy"))).ok()?;
    let (x0, y0) = ((w - size) / 2, (h - size) / 2);
    let mut planes = vec![vec![0.0f32; size * size]; 3];
    let mut disp = vec![0.0f32; size * size];
    let mut mask = vec![false; size * size];
    let mut vals: Vec<f32> = Vec::new();
    for y in 0..size {
        for x in 0..size {
            let mut logd_sum = 0.0f32;
            let mut logd_n = 0usize;
            let mut rgb_sum = [0.0f32; 3];
            let mut rgb_n = 0usize;
            if full {
                let (sx, sy) = (w / size, h / size);
                for by in 0..sy {
                    for bx in 0..sx {
                        let si = (y * sy + by) * w + x * sx + bx;
                        for c in 0..3 {
                            rgb_sum[c] += rgb[si * 3 + c];
                        }
                        rgb_n += 1;
                        let d = dep[si];
                        if d.is_finite() && d > 0.05 && d < 50.0 && seg[si] > 0 {
                            logd_sum += d.ln();
                            logd_n += 1;
                        }
                    }
                }
                for c in 0..3 {
                    planes[c][y * size + x] = (rgb_sum[c] / rgb_n.max(1) as f32 - 0.5) * 2.0;
                }
                // 前景占比 <25% 的块不参与训练（块里多为背景时均值没有意义）
                if logd_n * 4 >= rgb_n {
                    let dv = logd_sum / logd_n as f32;
                    disp[y * size + x] = dv;
                    mask[y * size + x] = true;
                    vals.push(dv);
                }
            } else {
                let si = (y0 + y) * w + x0 + x;
                for c in 0..3 {
                    // 输入标准化到约 [-1,1]：原值 0..1 在小 lr 下推不动
                    planes[c][y * size + x] = (rgb[si * 3 + c] - 0.5) * 2.0;
                }
                let d = dep[si];
                // 目标是 **log 深度**（比视差稳：近处不会放大成几百倍）
                if d.is_finite() && d > 0.05 && d < 50.0 && seg[si] > 0 {
                    let dv = d.ln();
                    disp[y * size + x] = dv;
                    mask[y * size + x] = true;
                    vals.push(dv);
                }
            }
        }
    }
    if vals.len() < size * size / 20 {
        return None;
    }
    Some((planes, disp, mask))
}

/// 只从 RGB 造输入平面（盒式降采样到 `size²`）——推理时不读 GT 深度/分割。
pub fn image_planes(p: &Path, size: usize) -> Option<Vec<Vec<f32>>> {
    let (w, h, rgb) = read_rgb(p).ok()?;
    if w < size || h < size {
        return None;
    }
    let (sx, sy) = (w / size, h / size);
    let mut planes = vec![vec![0.0f32; size * size]; 3];
    for y in 0..size {
        for x in 0..size {
            let mut s = [0.0f32; 3];
            for by in 0..sy {
                for bx in 0..sx {
                    let si = (y * sy + by) * w + x * sx + bx;
                    for c in 0..3 {
                        s[c] += rgb[si * 3 + c];
                    }
                }
            }
            let n = (sx * sy) as f32;
            for c in 0..3 {
                planes[c][y * size + x] = (s[c] / n - 0.5) * 2.0;
            }
        }
    }
    Some(planes)
}

/// `cnn-train --data <场景目录,…> --holdout a,b --size 64 --epochs 3 --out model.dcn`：
/// 三值卷积逐像素深度头（归一化视差），留出场景评估相对误差。
fn cnn_train(argv: &[String]) {
    use trit_parts::depthnet::DepthNet;
    let data: Vec<PathBuf> = arg(argv, "--data")
        .unwrap_or_else(|| usage())
        .split(',')
        .map(PathBuf::from)
        .collect();
    let size: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let epochs: usize = arg(argv, "--epochs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let hidden: usize = arg(argv, "--hidden")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let stride: usize = arg(argv, "--stride")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let holdout: Vec<String> = arg(argv, "--holdout")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();
    let full = argv.iter().any(|a| a == "--full");
    let ternary = !argv.iter().any(|a| a == "--f32");
    let dil: usize = arg(argv, "--dil").and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut net = DepthNet::with_ternary(hidden, 7, ternary);
    net.set_dilation(dil);
    println!(
        "卷积深度头：{} 权重，dil {dil}（感受野 {}）",
        if ternary { "三值" } else { "f32" },
        6 * dil + 1
    );
    type TrainSample = (PathBuf, usize, Vec<Vec<f32>>, Vec<f32>, Vec<bool>);
    let mut samples: Vec<TrainSample> = Vec::new();
    for d in &data {
        let mut fr = 0usize;
        while let Some((x, y, mk)) = load_sample(d, fr, size, full) {
            if fr.is_multiple_of(stride.max(1)) {
                samples.push((d.clone(), fr, x, y, mk));
            }
            fr += 1;
            if fr > 24 {
                break;
            }
        }
    }
    if samples.is_empty() {
        panic!("没有可用样本");
    }
    let is_held = |p: &Path| {
        let n = p.file_name().map(|s| s.to_string_lossy().into_owned());
        n.is_some_and(|n| holdout.iter().any(|h| n.ends_with(h.as_str())))
    };
    let (mut train, held): (Vec<_>, Vec<_>) = samples.iter().cloned().partition(|s| !is_held(&s.0));
    if train.is_empty() {
        panic!("留出后没有训练样本");
    }
    // **逐图**归一化：目标是形状（去掉相机距离），每图用自己的 log 深度中位/IQR。
    // 数据集级归一化实测把隐藏层 scale 压到 0.04（全局 DC 与 scale 耦合），训练塌成常数。
    // 推理没有「这张图自己的中位数」⇒ 用训练集的**平均**逐图统计（net.med/net.iqr）当默认尺度。
    let norm = |y: &mut [f32], mk: &[bool]| -> (f32, f32) {
        let mut v: Vec<f32> = y
            .iter()
            .zip(mk)
            .filter(|(_, m)| **m)
            .map(|(x, _)| *x)
            .collect();
        v.sort_by(f32::total_cmp);
        let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
        let (med, iqr) = (q(0.5), (q(0.75) - q(0.25)).max(1e-3));
        for (x, m) in y.iter_mut().zip(mk.iter()) {
            *x = if *m {
                ((*x - med) / iqr).clamp(-4.0, 4.0)
            } else {
                0.0
            };
        }
        (med, iqr)
    };
    let mut med_sum = 0.0f32;
    let mut iqr_sum = 0.0f32;
    for (_, _, _, y, mk) in train.iter_mut() {
        let (m, q) = norm(y, mk);
        med_sum += m;
        iqr_sum += q;
    }
    net.med = med_sum / train.len() as f32;
    net.iqr = iqr_sum / train.len() as f32;
    println!(
        "样本 {}（训练 {} / 留出 {}；{} 场景，{}²，hidden {}），逐图统计均值 med {:.4} iqr {:.4}",
        samples.len(),
        train.len(),
        held.len(),
        data.len(),
        size,
        hidden,
        net.med,
        net.iqr
    );
    if let Some((_, _, _, y0, mk0)) = samples.first() {
        let mut vals: Vec<f32> = y0
            .iter()
            .zip(mk0)
            .filter(|(_, m)| **m)
            .map(|(v, _)| *v)
            .collect();
        vals.sort_by(f32::total_cmp);
        println!(
            "首样本原始 log 深度：有效 {}/{} 像素，范围 [{:.2}, {:.2}]",
            vals.len(),
            size * size,
            vals.first().copied().unwrap_or(0.0),
            vals.last().copied().unwrap_or(0.0)
        );
    }
    for ep in 0..epochs {
        let t_ep = std::time::Instant::now();
        let mut loss = 0.0f32;
        for (_, _, x, y, mk) in &train {
            loss += net.train_step(x, size, size, y, mk, lr);
        }
        if ep.is_multiple_of(50) || ep + 1 == epochs {
            let nz = |c: &trit_parts::tconv::Conv| {
                c.w.iter().filter(|v| v.abs() > 0.5).count() as f32 / c.w.len().max(1) as f32
            };
            let am = |v: &[f32]| v.iter().sum::<f32>() / v.len().max(1) as f32;
            println!(
                "  诊断：|w| c1 {:.2} c2 {:.2} c3 {:.2}；非零 c1 {:.2} c2 {:.2}；scale c1 {:.2} c2 {:.2} c3 {:.2}",
                am(&net.c1.w.iter().map(|v| v.abs()).collect::<Vec<_>>()),
                am(&net.c2.w.iter().map(|v| v.abs()).collect::<Vec<_>>()),
                am(&net.c3.w.iter().map(|v| v.abs()).collect::<Vec<_>>()),
                nz(&net.c1),
                nz(&net.c2),
                am(&net.c1.scale),
                am(&net.c2.scale),
                am(&net.c3.scale),
            );
        }
        let (_, _, x0, _, _) = &train[0];
        let p0 = net.forward(x0, size, size);
        let (mut lo, mut hi, mut mean) = (f32::INFINITY, f32::NEG_INFINITY, 0.0f32);
        for v in &p0 {
            lo = lo.min(*v);
            hi = hi.max(*v);
            mean += v;
        }
        println!(
            "epoch {}/{}：MSE {:.4}；首训练样本预测 范围 [{lo:.2}, {hi:.2}] 均值 {:.2}（{:.0} ms）",
            ep + 1,
            epochs,
            loss / train.len() as f32,
            mean / p0.len().max(1) as f32,
            t_ep.elapsed().as_secs_f64() * 1000.0
        );
        let _ = &out;
    }
    net.save(&out).unwrap_or_else(|e| panic!("{e}"));
    println!("已存 {}", out.display());
    if !held.is_empty() {
        // 留出集是**原始** log 深度。两种口径：
        // (a) 形状对齐：用该图 GT 的中位/IQR 反归一（只量形状，尺度按定义对齐）；
        // (b) 可部署：用训练集平均逐图统计（net.med/net.iqr）——照片上没有这张图自己的中位数。
        let (mut mae_n, mut rel, mut rel_dep, mut rel_base, mut delta, mut n) =
            (0f64, 0f64, 0f64, 0f64, 0f64, 0usize);
        for (_, _, x, y, mk) in &held {
            let mut v: Vec<f32> = y
                .iter()
                .zip(mk)
                .filter(|(_, m)| **m)
                .map(|(a, _)| *a)
                .collect();
            v.sort_by(f32::total_cmp);
            let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
            let (gmed, giqr) = (q(0.5), (q(0.75) - q(0.25)).max(1e-3));
            let p = net.forward(x, size, size);
            for i in 0..size * size {
                if !mk[i] {
                    continue;
                }
                let raw = y[i];
                let aligned = p[i] * giqr + gmed;
                let dep = p[i] * net.iqr + net.med;
                let e = f64::from((aligned - raw).abs());
                mae_n += e;
                rel += (e.exp() - 1.0).abs();
                rel_dep += (f64::from(dep - raw).exp() - 1.0).abs();
                // 常数基线：预测该图 GT 中位深度（对形状完全无知）
                rel_base += (f64::from(raw - gmed).exp() - 1.0).abs();
                delta += f64::from(u8::from(e < 0.2231));
                n += 1;
            }
        }
        let k = n.max(1) as f64;
        println!(
            "留出 {} 帧 / {} 像素：形状对齐 log MAE {:.4}、深度相对误差 {:.2}%（常数基线 {:.2}%），δ<1.25 {:.3}；默认尺度相对误差 {:.2}%",
            held.len(),
            n,
            mae_n / k,
            rel / k * 100.0,
            rel_base / k * 100.0,
            delta / k,
            rel_dep / k * 100.0
        );
    }
}

/// 把 64² 的归一化深度预测双线性放大到 `w×h`，并按 `med/iqr` 还原成米。
fn dcn_depth_map(
    dn: &trit_parts::depthnet::DepthNet,
    planes: &[Vec<f32>],
    w: usize,
    h: usize,
    size: usize,
) -> Vec<f32> {
    let pred = dn.forward(planes, size, size);
    let mut d = vec![0.0f32; w * h];
    for y in 0..h {
        let fy = (y as f32 + 0.5) / h as f32 * size as f32 - 0.5;
        let y0 = fy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(size - 1);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..w {
            let fx = (x as f32 + 0.5) / w as f32 * size as f32 - 0.5;
            let x0 = fx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(size - 1);
            let tx = (fx - x0 as f32).clamp(0.0, 1.0);
            let v00 = pred[y0 * size + x0];
            let v01 = pred[y0 * size + x1];
            let v10 = pred[y1 * size + x0];
            let v11 = pred[y1 * size + x1];
            let v = v00 * (1.0 - tx) * (1.0 - ty)
                + v01 * tx * (1.0 - ty)
                + v10 * (1.0 - tx) * ty
                + v11 * tx * ty;
            d[y * w + x] = (v * dn.iqr + dn.med).exp();
        }
    }
    d
}

/// `depth-iou`：单图深度 → 3D 点云，与 GT 深度点云比体素 IoU。
/// 两个口径：(a) 绝对尺度（`net.med/iqr` 默认尺度）；(b) 形状对齐（按中位深度对齐尺度）。
fn depth_iou(argv: &[String]) {
    use trit_parts::geo::{Camera, iou3d, read_npy_f32, voxelize};

    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frame: usize = arg(argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let dcn_p = PathBuf::from(arg(argv, "--dcn").unwrap_or_else(|| usage()));
    let dcn_size: usize = arg(argv, "--dcn-size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.02);

    let p = scene.join("rgb").join(format!("{frame:06}.png"));
    let (w, h, _) = trit_parts::data::read_rgb(&p).unwrap_or_else(|e| panic!("{e}"));
    let (seg, _) =
        trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{frame:06}.npy")))
            .unwrap_or_else(|e| panic!("{e}"));
    let (gt, _) = read_npy_f32(&scene.join("depth").join(format!("{frame:06}.npy")))
        .unwrap_or_else(|e| panic!("{e}"));
    let cam = Camera::load(&scene.join("meta").join(format!("{frame:06}.json")))
        .unwrap_or_else(|e| panic!("{e}"));
    let dn = trit_parts::depthnet::DepthNet::load(&dcn_p).unwrap_or_else(|e| panic!("{e}"));
    let planes = image_planes(&p, dcn_size).unwrap_or_else(|| panic!("图像太小"));
    let pred = dcn_depth_map(&dn, &planes, w, h, dcn_size);

    let (mut gp, mut pg, mut ppd) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..w * h {
        if seg[i] <= 0 {
            continue;
        }
        let g = gt[i];
        let d = pred[i];
        if !(g.is_finite() && g > 0.05 && g < 50.0 && d.is_finite() && d > 0.05 && d < 50.0) {
            continue;
        }
        let (x, y) = ((i % w), (i / w));
        gp.push([x as f32, y as f32]);
        pg.push(g);
        ppd.push(d);
    }
    let un = |pix: &[[f32; 2]], d: &[f32]| -> Vec<[f32; 3]> {
        pix.iter()
            .zip(d)
            .map(|(q, z)| cam.unproject(q[0] as f64, q[1] as f64, *z))
            .collect()
    };
    let gpts = un(&gp, &pg);
    let ppts = un(&gp, &ppd);
    let mut gs: Vec<f32> = pg.clone();
    let mut ps: Vec<f32> = ppd.clone();
    gs.sort_by(f32::total_cmp);
    ps.sort_by(f32::total_cmp);
    let (gmed, pmed) = (gs[gs.len() / 2], ps[ps.len() / 2]);
    let scale = gmed / pmed;
    let ppts_s: Vec<[f32; 3]> = ppts
        .iter()
        .map(|q| [q[0] * scale, q[1] * scale, q[2] * scale])
        .collect();
    let iou_abs = iou3d(&voxelize(&gpts, cell), &voxelize(&ppts, cell));
    let iou_shape = iou3d(&voxelize(&gpts, cell), &voxelize(&ppts_s, cell));
    let rel_abs = ppd
        .iter()
        .zip(&pg)
        .map(|(d, g)| (d / g - 1.0).abs())
        .sum::<f32>()
        / pg.len().max(1) as f32;
    let rel_shape = ppd
        .iter()
        .zip(&pg)
        .map(|(d, g)| (d * scale / g - 1.0).abs())
        .sum::<f32>()
        / pg.len().max(1) as f32;
    println!(
        "{} 帧 {frame}：前景 {} 点；中位深度 GT {:.2} m / 预测 {:.2} m（尺度比 {:.2}）；\
         相对误差 绝对 {:.2}% / 形状对齐 {:.2}%；体素 IoU（cell {cell}）绝对 {:.3} / 形状对齐 {:.3}",
        scene.display(),
        pg.len(),
        gmed,
        pmed,
        scale,
        rel_abs * 100.0,
        rel_shape * 100.0,
        iou_abs,
        iou_shape
    );
}

fn mesh_single(argv: &[String]) {
    use trit_parts::mesh::{boundary_edges, fill_solid, surface_nets, write_ply};

    let model = arg(argv, "--model").map(PathBuf::from);
    let head_p = arg(argv, "--head").map(PathBuf::from);
    let dcn_p = arg(argv, "--dcn").map(PathBuf::from);
    let dcn_size: usize = arg(argv, "--dcn-size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let scene = PathBuf::from(arg(argv, "--scene").unwrap_or_else(|| usage()));
    let frame: usize = arg(argv, "--frame")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let cell: f32 = arg(argv, "--cell")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.02);
    let use_gt = argv.iter().any(|a| a == "--gt");
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));

    let p = scene.join("rgb").join(format!("{frame:06}.png"));
    let (w, h, _) = trit_parts::data::read_rgb(&p).unwrap_or_else(|e| panic!("{e}"));
    // 前景遮罩：有 GT 分割就用它；真实照片没有 seg 文件，先退化成全图
    // （无 GT 的自动抠图属于分割那一块，见文档缺口）。
    let mask: Vec<bool> =
        match trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{frame:06}.npy"))) {
            Ok((seg, _)) => seg.iter().map(|&v| v > 0).collect(),
            Err(_) => vec![true; w * h],
        };

    let depth: Vec<f32> = if use_gt {
        let (d, _) =
            trit_parts::geo::read_npy_f32(&scene.join("depth").join(format!("{frame:06}.npy")))
                .unwrap_or_else(|e| panic!("{e}"));
        d
    } else if let Some(dp) = dcn_p {
        // 卷积深度头：整图降采样 → 逐像素归一化深度 → 双线性回原分辨率
        let dn = trit_parts::depthnet::DepthNet::load(&dp).unwrap_or_else(|e| panic!("{e}"));
        let planes = image_planes(&p, dcn_size)
            .unwrap_or_else(|| panic!("图像小于 {dcn_size}²，无法降采样"));
        dcn_depth_map(&dn, &planes, w, h, dcn_size)
    } else {
        let gpu = Gpu::new().expect("GPU 初始化失败");
        let tpm = TpmModel::load(&model.unwrap_or_else(|| panic!("预测模式需要 --model 或 --dcn")))
            .unwrap_or_else(|e| panic!("{e}"));
        let mut net = Net::from_tpm(&tpm);
        net.upload_all(&gpu);
        let head =
            DepthHead::load(&head_p.unwrap_or_else(|| panic!("预测模式需要 --head 或 --dcn")))
                .unwrap_or_else(|e| panic!("{e}"));
        let (feats, _, npix) = forward_all(&net, &gpu, &p);
        let mut disp = head.disparity(&feats, npix);
        // 逐图中位数对齐（默认关）：实测把它打开后单图重建与 GT 的 IoU 从 0.016 掉到 0.001，
        // 说明误差不是简单的整体偏移，而是逐像素的非线性偏差 ⇒ 仅保留为实验开关。
        if argv.iter().any(|a| a == "--align-median") {
            let mut fg: Vec<f32> = (0..npix).filter(|&i| mask[i]).map(|i| disp[i]).collect();
            fg.sort_by(f32::total_cmp);
            if !fg.is_empty() {
                let med = fg[fg.len() / 2];
                let shift = head.b - med;
                for v in disp.iter_mut() {
                    *v += shift;
                }
                println!(
                    "视差中位对齐：{med:.4} → {:.4}（shift {shift:+.4}）",
                    head.b
                );
            }
        }
        DepthHead::depth(&disp)
    };

    let cam = trit_parts::geo::Camera::load(&scene.join("meta").join(format!("{frame:06}.json")))
        .unwrap_or_else(|e| panic!("{e}"));
    let mut pts: Vec<[f32; 3]> = Vec::new();
    for i in 0..w * h {
        if !mask[i] {
            continue;
        }
        let d = depth[i];
        if !(d.is_finite() && d > 0.05 && d < 50.0) {
            continue;
        }
        pts.push(cam.unproject((i % w) as f64, (i / w) as f64, d));
    }
    let grid =
        trit_parts::mesh::Grid::from_points(&pts, cell, 3).unwrap_or_else(|| panic!("无有效点"));
    let mut g = grid.clone();
    fill_solid(&mut g);
    let m = surface_nets(&g);
    write_ply(&out, &m).unwrap_or_else(|e| panic!("{e}"));
    println!(
        "单图{} → 点 {} / 顶点 {} / 面 {} / 边界边 {} → {}",
        if use_gt {
            "（GT 深度）"
        } else {
            "（预测深度）"
        },
        pts.len(),
        m.verts.len(),
        m.tris.len(),
        boundary_edges(&m),
        out.display()
    );
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
  parts-depth eval --model <tpm> --head <tph> --scene <dir> [--limit N]
  parts-depth cnn-train --data <dir,...> --holdout a,b --size 64 --epochs 60 --out <dcn>
  parts-depth mesh-single [--dcn <dcn> | --gt] --scene <dir> --frame N --out <ply>"
    );
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("train") => train(&argv),
        Some("eval") => eval(&argv),
        Some("mesh-single") => mesh_single(&argv),
        Some("cnn-train") => cnn_train(&argv),
        Some("depth-iou") => depth_iou(&argv),
        _ => usage(),
    }
}
