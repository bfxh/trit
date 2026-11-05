use std::path::PathBuf;

use trit_parts::data::{frames, load_frame_sub};
use trit_parts::gpu::Gpu;
use trit_parts::net::Net;
use trit_parts::parts::{PartList, evaluate_frame};
use trit_parts::weights::Tensors;

use super::args::Args;
use super::selfcheck::expand_scenes;

/// 评估：同一模型分别用「无约束 argmax」与「清单约束 argmax」解码，指标并排。
pub(crate) fn eval_only(gpu: &Gpu, args: &Args) {
    let scenes: Vec<PathBuf> = args
        .eval
        .as_ref()
        .expect("--eval")
        .to_string_lossy()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .flat_map(|p| expand_scenes(&p))
        .collect();
    let manifest_path = args.manifest.as_ref().unwrap_or_else(|| {
        eprintln!("--eval 需要 --manifest <parts.json>");
        std::process::exit(2);
    });
    let m = PartList::load(manifest_path).unwrap_or_else(|e| {
        eprintln!("清单读取失败: {e}");
        std::process::exit(1);
    });
    let k = args.k;
    if m.k != k {
        eprintln!(
            "注意：清单 k={} 与模型 k={} 不一致；解码按模型 k，清单只用于允许 id 集",
            m.k, k
        );
    }
    println!("清单 k={k}，部件 {} 个", m.parts.len());

    let mut net = match &args.resume {
        Some(p) => trit_parts::state::load(p).map(|(n, _, _)| n),
        None => match &args.init {
            Some(dir) => Tensors::load(dir).and_then(|t| Net::from_tensors(&t)),
            None => {
                eprintln!("--eval 需要 --resume 或 --init");
                std::process::exit(2);
            }
        },
    }
    .unwrap_or_else(|e| {
        eprintln!("模型加载失败: {e}");
        std::process::exit(1);
    });
    net.upload_all(gpu);
    let ch = net.head_cout() as usize;

    let res = args.res;
    let hw = (res * res) as usize;

    let mut plane_sum = vec![0f64; k];
    // [无约束, 约束]：帧级累加（id 一致按帧取均值，IoU 按 (帧,部件) 对取均值，清单外按帧取均值）
    let mut frames_n = 0usize;
    let mut exact = [0f64; 2];
    let mut exact_n = [0usize; 2];
    let mut iou = [0f64; 2];
    let mut iou_n = [0usize; 2];
    let mut unk_ids = [0f64; 2];
    let mut unk_px = [0f64; 2];
    let mut unk_px_n = [0f64; 2];
    let mut dep_abs = 0f64;
    let mut dep_rel = 0f64;
    let mut dep_d1 = 0f64;
    let mut dep_n = 0usize;

    for scene in &scenes {
        let mut paths = frames(scene);
        let cap = if args.limit > 0 {
            args.limit
        } else {
            usize::MAX
        };
        paths.truncate(cap);
        for (n, file) in paths.iter().enumerate() {
            let s = load_frame_sub(scene, file, k, &args.seg_sub).unwrap_or_else(|e| {
                eprintln!("读帧失败: {e}");
                std::process::exit(1);
            });
            let xb = gpu.buf(&s.rgb, "x");
            let tr = net.forward(gpu, xb, res);
            let logits = gpu.read(&tr.out, hw * ch);
            frames_n += 1;
            if let Some(buf) = &tr.depth_out {
                if args.depth_planes == 0 {
                    eprintln!("--eval 读带深度头的模型需要 --depth-planes");
                    std::process::exit(2);
                }
                let dplanes = args.depth_planes;
                let dstride = net.depth_cout() as usize;
                let levels = 3usize.pow(dplanes as u32) - 1;
                let dl = gpu.read(buf, hw * dstride);
                let stem = file
                    .file_stem()
                    .map(|v| v.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (dep, _) =
                    trit_parts::geo::read_npy_f32(&scene.join("depth").join(format!("{stem}.npy")))
                        .unwrap_or_else(|e| {
                            eprintln!("读深度失败: {e}");
                            std::process::exit(1);
                        });
                for (p, &gd) in dep.iter().enumerate().take(hw) {
                    if s.valid[p] == 0 {
                        continue;
                    }
                    if !(gd.is_finite() && gd > 0.05 && gd < 50.0) {
                        continue;
                    }
                    let mut lvl = 0usize;
                    for i in 0..dplanes {
                        let b = p * dstride + i * 3;
                        let z = [dl[b], dl[b + 1], dl[b + 2]];
                        let mut best = 0usize;
                        for c in 1..3 {
                            if z[c] > z[best] {
                                best = c;
                            }
                        }
                        lvl = lvl * 3 + best;
                    }
                    let t = lvl as f32 / levels.max(1) as f32;
                    let disp = args.depth_lo + t * (args.depth_hi - args.depth_lo);
                    let pd = 1.0 / disp.max(1e-3);
                    let e = (pd - gd).abs() as f64;
                    dep_abs += e;
                    dep_rel += e / gd as f64;
                    dep_d1 += f64::from(u8::from((pd.max(gd) / pd.min(gd)) < 1.25));
                    dep_n += 1;
                }
            }
            if let Some(dir) = &args.dump {
                std::fs::create_dir_all(dir).unwrap_or_else(|e| {
                    eprintln!("{}: {e}", dir.display());
                    std::process::exit(1);
                });
                // 真实 stride = logits.len()/hw（含末尾 valid 通道），不是 k*3
                let stride = logits.len() / hw;
                let mut out = vec![0i32; hw];
                for (p, slot) in out.iter_mut().enumerate() {
                    let base = p * stride;
                    *slot = m.decode_constrained(&logits[base..base + k * 3]) as i32;
                }
                let name = format!(
                    "{}_{:06}.npy",
                    scene.file_name().unwrap_or_default().to_string_lossy(),
                    n
                );
                if let Err(e) = trit_parts::data::write_npy_i32(
                    &dir.join(name),
                    &out,
                    res as usize,
                    res as usize,
                ) {
                    eprintln!("写预测失败: {e}");
                    std::process::exit(1);
                }
            }
            for (slot, constrained) in [false, true].into_iter().enumerate() {
                let fm = evaluate_frame(&logits, &s.planes, &s.valid, &m, constrained, true);
                if !constrained {
                    for (acc, v) in fm.plane_acc.iter().enumerate() {
                        plane_sum[acc] += f64::from(*v);
                    }
                }
                if let Some(e) = fm.id_exact {
                    exact[slot] += f64::from(e);
                    exact_n[slot] += 1;
                }
                for v in &fm.part_iou {
                    iou[slot] += f64::from(*v);
                    iou_n[slot] += 1;
                }
                let (bad, total) = fm.unknown_ids;
                if total > 0 {
                    unk_ids[slot] += bad as f64 / total as f64;
                }
                let (up, vp) = fm.unknown_pixels;
                unk_px[slot] += up as f64;
                unk_px_n[slot] += vp as f64;
            }
            if (n + 1) % 50 == 0 || n + 1 == paths.len() {
                println!("  {}: {}/{}", scene.display(), n + 1, paths.len());
            }
        }
    }

    let nf = frames_n as f64;
    let planes: Vec<f64> = plane_sum.iter().map(|v| v / nf).collect();
    let mean = |s: f64, n: usize| if n > 0 { s / n as f64 } else { 0.0 };
    let plane_show: Vec<f64> = planes
        .iter()
        .map(|v| (v * 1000.0).round() / 1000.0)
        .collect();
    println!("帧数 {frames_n}");
    if dep_n > 0 {
        println!(
            "深度（预测 vs GT）：MAE {:.1} mm，相对误差 {:.2}%，δ<1.25 {:.3}（{} 像素）",
            dep_abs / dep_n as f64 * 1000.0,
            dep_rel / dep_n as f64 * 100.0,
            dep_d1 / dep_n as f64,
            dep_n
        );
    }
    println!("平面准确率(GT 部件像素) {plane_show:?}");
    println!("{:<18}{:>10}{:>10}", "指标", "无约束", "清单约束");
    println!(
        "{:<18}{:>10.4}{:>10.4}",
        "id 一致",
        mean(exact[0], exact_n[0]),
        mean(exact[1], exact_n[1])
    );
    println!(
        "{:<18}{:>10.4}{:>10.4}",
        "部件 IoU",
        mean(iou[0], iou_n[0]),
        mean(iou[1], iou_n[1])
    );
    println!(
        "{:<18}{:>10.3}{:>10.3}",
        "清单外 id 率",
        unk_ids[0] / nf,
        unk_ids[1] / nf
    );
    println!(
        "{:<18}{:>10.4}{:>10.4}",
        "清单外像素占比",
        if unk_px_n[0] > 0.0 {
            unk_px[0] / unk_px_n[0]
        } else {
            0.0
        },
        if unk_px_n[1] > 0.0 {
            unk_px[1] / unk_px_n[1]
        } else {
            0.0
        }
    );
}
