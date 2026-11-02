//! `parts-depth`：冻结骨干 + f32 线性深度头（视差）的训练与评估。
//!
//! - `train --model <tpm> --data <dir,...> --steps N --lr 0.05 --save head.tpdh`
//! - `eval --model <tpm> --head head.tpdh --scene <dir> [--limit N] [--3d --manifest parts.json]`

use std::path::{Path, PathBuf};

// 二进制 crate root 的 `mod x;` 只找同目录 `x.rs`（同目录下每个 .rs 都会被 Cargo 当成 bin），
// 所以子模块必须显式给路径。
#[path = "parts_depth/depth_train.rs"]
mod depth_train;

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
    // 前景遮罩优先级：`--fg <tpm>`（模型前景头，真实照片用）> GT `seg/` > 全图。
    let mask: Vec<bool> = if let Some(fp) = arg(argv, "--fg").map(PathBuf::from) {
        let k: usize = arg(argv, "--fg-k")
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);
        let gpu = Gpu::new().expect("GPU 初始化失败");
        let tpm = TpmModel::load(&fp).unwrap_or_else(|e| panic!("{e}"));
        let mut net = Net::from_tpm(&tpm);
        net.upload_all(&gpu);
        let (_, logits, npix) = forward_all(&net, &gpu, &p);
        let ch = logits.len() / npix.max(1);
        if k * 3 >= ch {
            panic!("模型输出 {ch} 通道，不够 k={k}（需 {}）", k * 3 + 1);
        }
        let m: Vec<bool> = (0..npix).map(|i| logits[i * ch + k * 3] > 0.0).collect();
        let fg = m.iter().filter(|v| **v).count() as f32 / npix as f32;
        if let Ok((gt, _)) =
            trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{frame:06}.npy")))
        {
            let (mut inter, mut union) = (0usize, 0usize);
            for i in 0..npix {
                let g = gt[i] > 0;
                inter += (m[i] && g) as usize;
                union += (m[i] || g) as usize;
            }
            println!(
                "前景头：覆盖率 {:.1}%，前景 IoU {:.3}",
                fg * 100.0,
                inter as f32 / union.max(1) as f32
            );
        } else {
            println!("前景头：覆盖率 {:.1}%（无 GT seg 可比）", fg * 100.0);
        }
        m
    } else {
        match trit_parts::data::read_npy_i32(&scene.join("seg").join(format!("{frame:06}.npy"))) {
            Ok((seg, _)) => seg.iter().map(|&v| v > 0).collect(),
            Err(_) => vec![true; w * h],
        }
    };

    let depth: Vec<f32> = if use_gt {
        let (d, _) =
            trit_parts::geo::read_npy_f32(&scene.join("depth").join(format!("{frame:06}.npy")))
                .unwrap_or_else(|e| panic!("{e}"));
        d
    } else if let Some(dp) = dcn_p {
        // 卷积深度头：整图降采样 → 逐像素归一化深度 → 双线性回原分辨率
        let dn = trit_parts::depthnet::DepthNet::load(&dp).unwrap_or_else(|e| panic!("{e}"));
        let planes = depth_train::image_planes(&p, dcn_size, (dn.c1.cin.saturating_sub(3)) / 3)
            .unwrap_or_else(|| panic!("图像小于 {dcn_size}²，无法降采样"));
        depth_train::dcn_depth_map(&dn, &planes, w, h, dcn_size)
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

    if let Some(ip) = arg(argv, "--inst").map(PathBuf::from) {
        let split_dir = PathBuf::from(arg(argv, "--split-dir").unwrap_or_else(|| ".".to_string()));
        let min_tris: usize = arg(argv, "--min-tris")
            .and_then(|v| v.parse().ok())
            .unwrap_or(10);
        split_mesh_by_instances(&m, &cam, &ip, w, h, &split_dir, min_tris);
    }
}

/// 把网格按**源视角的 2D 实例图**切片：每个顶点投回图像取最近像素的实例 id，
/// 每个面取三顶点的多数票；按 id 分组重映射顶点写成独立 PLY。
fn split_mesh_by_instances(
    m: &trit_parts::mesh::Mesh,
    cam: &trit_parts::geo::Camera,
    inst_path: &Path,
    w: usize,
    h: usize,
    out_dir: &Path,
    min_tris: usize,
) {
    use std::collections::BTreeMap;
    let (inst, shape) = trit_parts::data::read_npy_i32(inst_path).unwrap_or_else(|e| panic!("{e}"));
    if shape.len() != 2 || shape[1] != w || shape[0] != h {
        panic!(
            "实例图 {}×{} 与图像 {w}×{h} 不一致",
            shape.get(1).copied().unwrap_or(0),
            shape.first().copied().unwrap_or(0)
        );
    }
    // 顶点 → 实例（0 = 未命中/背景）
    let vid: Vec<u32> = m
        .verts
        .iter()
        .map(|p| match cam.project(*p) {
            Some((u, v, _)) => {
                let (ux, vy) = (u.round(), v.round());
                if ux < 0.0 || vy < 0.0 || ux >= w as f64 || vy >= h as f64 {
                    0
                } else {
                    inst[vy as usize * w + ux as usize].max(0) as u32
                }
            }
            None => 0,
        })
        .collect();
    let mut groups: BTreeMap<u32, Vec<[u32; 3]>> = BTreeMap::new();
    for t in &m.tris {
        let (a, b, c) = (vid[t[0] as usize], vid[t[1] as usize], vid[t[2] as usize]);
        let id = if a == b || a == c {
            a
        } else if b == c {
            b
        } else {
            0
        };
        groups.entry(id).or_default().push(*t);
    }
    std::fs::create_dir_all(out_dir).ok();
    let mut written = 0usize;
    let mut skipped = 0usize;
    let mut sizes: Vec<usize> = Vec::new();
    for (id, tris) in &groups {
        if *id == 0 {
            continue;
        }
        if tris.len() < min_tris {
            skipped += 1;
            continue;
        }
        let mut remap: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut verts: Vec<[f32; 3]> = Vec::new();
        let mut out_tris: Vec<[u32; 3]> = Vec::with_capacity(tris.len());
        for t in tris {
            let mut nt = [0u32; 3];
            for (k, &v) in t.iter().enumerate() {
                let nv = *remap.entry(v).or_insert_with(|| {
                    verts.push(m.verts[v as usize]);
                    verts.len() as u32 - 1
                });
                nt[k] = nv;
            }
            out_tris.push(nt);
        }
        let part = trit_parts::mesh::Mesh {
            verts,
            tris: out_tris,
        };
        let p = out_dir.join(format!("part_{id:03}.ply"));
        trit_parts::mesh::write_ply(&p, &part).unwrap_or_else(|e| panic!("{e}"));
        written += 1;
        sizes.push(tris.len());
    }
    sizes.sort_unstable();
    println!(
        "部件切片：{} 个（跳过 <{min_tris} 面的 {} 个；未归属面 {}）→ {}",
        written,
        skipped,
        groups.get(&0).map(Vec::len).unwrap_or(0),
        out_dir.display()
    );
    if let Some(mid) = sizes.get(sizes.len() / 2) {
        println!(
            "每部件面数：最小 {} / 中位 {} / 最大 {}",
            sizes.first().copied().unwrap_or(0),
            mid,
            sizes.last().copied().unwrap_or(0)
        );
    }
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
        Some("cnn-train") => depth_train::cnn_train(&argv),
        Some("depth-iou") => depth_train::depth_iou(&argv),
        _ => usage(),
    }
}
