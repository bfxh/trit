//! 深度头的训练与 3D 评测（从 `parts_depth.rs` 拆出，模块预算 800 行）。

use std::path::{Path, PathBuf};

use trit_parts::data::read_rgb;

use super::{arg, usage};

/// 一条训练样本：RGB 平面 / **原始 log 深度** / 有效掩码。
/// 归一化统计是**数据集级**的（存进 `DepthNet::med/iqr`）：逐图归一化会把绝对尺度抹掉，
/// 推理时拿不到「这张图自己的中位数」，网格就没有尺度。
type DepthSample = (Vec<Vec<f32>>, Vec<f32>, Vec<bool>);

/// 读一张图 + 深度，返回 `(rgb 平面, 原始 log 深度, 掩码)`。
///
/// `full=true` 时**整图盒式降采样**到 `size²`（前景只占 ~15%，中心裁剪会丢掉全局布局；
/// 64² 整图是 6×6 块均值）；否则中心裁剪。
fn load_sample(
    scene: &Path,
    frame: usize,
    size: usize,
    full: bool,
    pyr: usize,
) -> Option<DepthSample> {
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
    for lvl in 1..=pyr {
        add_lowpass(&mut planes, size, 4usize.pow(lvl as u32));
    }
    Some((planes, disp, mask))
}

/// 只从 RGB 造输入平面（盒式降采样到 `size²`）——推理时不读 GT 深度/分割。
pub(crate) fn image_planes(p: &Path, size: usize, pyr: usize) -> Option<Vec<Vec<f32>>> {
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
    for lvl in 1..=pyr {
        add_lowpass(&mut planes, size, 4usize.pow(lvl as u32));
    }
    Some(planes)
}

/// 多尺度输入：把每通道按 `factor` 做块均值再复制回原分辨率（`--pyr`）。
pub(crate) fn add_lowpass(planes: &mut Vec<Vec<f32>>, size: usize, factor: usize) {
    let n = planes.len();
    let s = size / factor;
    for c in 0..n {
        let src = &planes[c];
        let mut coarse = vec![0.0f32; s * s];
        for y in 0..s {
            for x in 0..s {
                let mut acc = 0.0f32;
                for by in 0..factor {
                    for bx in 0..factor {
                        acc += src[(y * factor + by) * size + x * factor + bx];
                    }
                }
                coarse[y * s + x] = acc / (factor * factor) as f32;
            }
        }
        let mut out = vec![0.0f32; size * size];
        for y in 0..size {
            for x in 0..size {
                out[y * size + x] = coarse[(y / factor) * s + x / factor];
            }
        }
        planes.push(out);
    }
}

/// `cnn-train --data <场景目录,…> --holdout a,b --size 64 --epochs 3 --out model.dcn`：
/// 三值卷积逐像素深度头（归一化视差），留出场景评估相对误差。
pub(crate) fn cnn_train(argv: &[String]) {
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
    let out = arg(argv, "--out").map(PathBuf::from);
    let holdout: Vec<String> = arg(argv, "--holdout")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();
    let full = argv.iter().any(|a| a == "--full");
    let eval_only = arg(argv, "--eval-only").map(PathBuf::from);
    let pyr: usize = arg(argv, "--pyr").and_then(|v| v.parse().ok()).unwrap_or(0);
    let ternary = !argv.iter().any(|a| a == "--f32");
    let dil: usize = arg(argv, "--dil").and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut net = DepthNet::with_inputs(3 + 3 * pyr, hidden, 7, ternary);
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
        while let Some((x, y, mk)) = load_sample(d, fr, size, full, pyr) {
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
    // 留出项可能给的是「目录名」也可能给的是完整路径（Git Bash 下 basename 不一定剥掉盘符）
    // ⇒ 一律取最后一段再比。
    let is_held = |p: &Path| {
        let n = p.file_name().map(|s| s.to_string_lossy().into_owned());
        n.is_some_and(|n| {
            holdout.iter().any(|h| {
                let hb = h.trim_end_matches(['/', '\\']);
                let hb = hb.rsplit(['/', '\\']).next().unwrap_or(hb);
                n == hb
            })
        })
    };
    let (mut train, held): (Vec<_>, Vec<_>) = samples.iter().cloned().partition(|s| !is_held(&s.0));
    if std::env::var("DEPTH_DEBUG").is_ok() {
        println!("holdout 清单 {holdout:?}");
        println!(
            "首个样本目录名 {:?}",
            samples.first().map(|s| s.0.file_name())
        );
    }
    if train.is_empty() && eval_only.is_none() {
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
    if let Some(p) = &eval_only {
        net = trit_parts::depthnet::DepthNet::load(p).unwrap_or_else(|e| panic!("{e}"));
        println!("仅评估 {}（跳过训练）", p.display());
    }
    for ep in 0..if eval_only.is_some() { 0 } else { epochs } {
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
    }
    if eval_only.is_none() {
        let out = out.unwrap_or_else(|| usage());
        net.save(&out).unwrap_or_else(|e| panic!("{e}"));
        println!("已存 {}", out.display());
    }
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
pub(crate) fn dcn_depth_map(
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
pub(crate) fn depth_iou(argv: &[String]) {
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
    let planes = image_planes(&p, dcn_size, dn.c1.cin.saturating_sub(3) / 3)
        .unwrap_or_else(|| panic!("图像太小"));
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
