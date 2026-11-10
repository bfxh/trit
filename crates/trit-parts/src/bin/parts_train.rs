//! 训练循环：QAT（三值权重 + STE），全流程 Rust + GPU。
//!
//! 损失口径与 Python 基线一致：层级加权 CE（k 条平面）+ 0.5·BCE（valid），
//! Adam/SGD，支持梯度累加与断点续跑。
//!
//! 例（4 帧过拟合自检）：
//! ```text
//! parts-train --data D:/KF/trit-data/drift-data/big-r2.4 --init D:/KF/trit-data/parts-parity \
//!   --limit 4 --steps 120 --lr 1e-3 --report 20
//! ```

use std::path::PathBuf;

use trit_parts::data::{Sample, load_frame_sub};
use trit_parts::gpu::Gpu;
use trit_parts::loss::{depth_loss_grad, loss_and_grad, plane_weights};
use trit_parts::net::Net;
use trit_parts::weights::Tensors;

#[path = "parts_train/args.rs"]
mod args;
#[path = "parts_train/eval.rs"]
mod eval;
#[path = "parts_train/selfcheck.rs"]
mod selfcheck;

use args::parse;
use eval::eval_only;
use selfcheck::selfcheck;

fn main() {
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("参数错误: {e}");
            std::process::exit(2);
        }
    };
    if let Some(dir) = &args.selfcheck {
        selfcheck(dir);
        return;
    }
    let gpu = match Gpu::new() {
        Some(g) => g,
        None => {
            eprintln!("没有可用的 GPU 适配器");
            std::process::exit(2);
        }
    };

    if args.eval.is_some() {
        eval_only(&gpu, &args);
        return;
    }

    // --state DIR：存在 state.bin 就续跑，否则按 --init/--random 新建；始终存回这里
    let mut resume_path = args.resume.clone();
    let mut save_path = args.save.as_ref().map(|dir| {
        if dir.extension().is_some() {
            dir.clone()
        } else {
            dir.join("state.bin")
        }
    });
    if let Some(dir) = &args.state {
        std::fs::create_dir_all(dir).ok();
        let p = dir.join("state.bin");
        if p.exists() {
            println!("发现进程存档，自动续跑：{}", p.display());
            resume_path = Some(p.clone());
        }
        save_path = Some(p);
    }

    // 数据：按目录列帧，--limit 截断。`--data` 可以是单场景（含 rgb/），
    // 也可以是「场景的父目录」（其子目录各含 rgb/）。
    let paths = trit_parts::partsrun::expand_frames(&args.data, args.limit);
    if paths.is_empty() {
        eprintln!("没有可用帧");
        std::process::exit(2);
    }

    let k = args.k;
    let res = args.res;
    let hw = (res * res) as usize;

    let mut resumed_step = 0u64;
    let mut resumed_rng: Option<u64> = None;
    let mut net = match &resume_path {
        Some(p) => match trit_parts::state::load(p) {
            Ok((n, step, rng)) => {
                match rng {
                    Some(_) => println!("续跑自 {}（step {step}，RNG 已恢复）", p.display()),
                    None => println!(
                        "续跑自 {}（step {step}；老存档无 RNG，取样顺序重排）",
                        p.display()
                    ),
                }
                resumed_step = step;
                resumed_rng = rng;
                n
            }
            Err(e) => {
                eprintln!("断点读取失败: {e}");
                std::process::exit(1);
            }
        },
        None => match &args.init {
            Some(dir) => {
                let t = Tensors::load(dir).unwrap_or_else(|e| {
                    eprintln!("初始权重读取失败: {e}");
                    std::process::exit(1)
                });
                Net::from_tensors(&t).unwrap_or_else(|e| {
                    eprintln!("建网失败: {e}");
                    std::process::exit(1)
                })
            }
            None => {
                if (args.width - 1.0).abs() > 1e-6 {
                    println!(
                        "从零训练，宽度 {:.2}×（通道 {} / {} / {}）",
                        args.width,
                        ((32.0 * args.width).round() as u32).div_ceil(4) * 4,
                        ((64.0 * args.width).round() as u32).div_ceil(4) * 4,
                        ((128.0 * args.width).round() as u32).div_ceil(4) * 4
                    );
                }
                Net::random_w(args.seed, args.width)
            }
        },
    };
    if let Some(src) = &args.expand_from {
        let (old, _, _) = trit_parts::state::load(src).unwrap_or_else(|e| {
            eprintln!("扩展源读取失败: {e}");
            std::process::exit(1);
        });
        net = old.expand_width(args.width);
        resumed_step = 0;
        println!(
            "net2net：从 {} 扩展为宽度 {:.2}×（新通道 E=0，保持原输出）",
            src.display(),
            args.width
        );
    }
    if args.depth_planes > 0 && net.depth_cout() == 0 {
        net.add_depth_head(args.depth_planes as u32, args.seed);
        println!(
            "独立深度头：{} 条平面（{} 级），输入 {} 通道、输出 {} 通道",
            args.depth_planes,
            3usize.pow(args.depth_planes as u32),
            net.layers[trit_parts::net::D1B].cout,
            net.depth_cout()
        );
    }
    net.head_lr_mult = args.head_lr;
    if args.unfreeze_last > 0 {
        net.freeze_until = net.layers.len().saturating_sub(args.unfreeze_last);
        println!(
            "只训练最后 {} 层（从第 {} 层起）",
            args.unfreeze_last, net.freeze_until
        );
    }
    net.freeze_head = args.freeze_head;
    if args.freeze {
        net.freeze_until = net.layers.len() - 1;
        println!("冻结骨干：只更新第 {} 层（深度头）", net.freeze_until);
    }
    if args.residual {
        net.set_residual(true);
        println!("块内残差：开（e1b/e2b/e3b/d2b/d1b）");
    }
    net.upload_all(&gpu);
    let ch = net.head_cout() as usize;
    let w = plane_weights(k);
    println!(
        "帧 {}，k={k}，res={res}，lr={}，accum={}，opt={}，ste={:?}",
        paths.len(),
        args.lr,
        args.accum,
        if args.adam { "adam" } else { "sgd" },
        args.ste
    );

    if let Some(p) = &save_path
        && let Some(parent) = p.parent()
    {
        std::fs::create_dir_all(parent).ok();
    }
    let mut aug = trit_parts::augment::Augment::new(args.seed ^ 0x5eed, args.augment_shift);
    let mut rng = resumed_rng.unwrap_or(args.seed | 1);
    let mut best = f32::INFINITY;
    let mut last_loss = f32::NAN;
    let mut ma = f32::NAN; // 损失的指数移动平均（单帧噪声大，直接打最后一帧会误导）
    let mut dep_out: Option<trit_parts::loss::DepthOut>;
    for step in 0..args.steps {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let (scene, file) = &paths[(rng % paths.len() as u64) as usize];
        let mut s: Sample = match load_frame_sub(scene, file, k, &args.seg_sub) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("读帧失败: {e}");
                std::process::exit(1);
            }
        };
        if args.augment {
            aug.apply(res, &mut s);
        }
        let dep = if args.depth_planes > 0 {
            match trit_parts::data::depth_planes(
                scene,
                file,
                hw,
                args.depth_planes,
                args.depth_lo,
                args.depth_hi,
            ) {
                Ok(v) => Some(v),
                Err(e) => {
                    eprintln!("读深度失败: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            None
        };
        let xb = gpu.buf(&s.rgb, "x");
        if args.batch {
            gpu.begin_batch();
        }
        let tr = net.forward(&gpu, xb, res);
        let logits = gpu.read(&tr.out, hw * ch);
        let out = if args.id_weight > 0.0 {
            // 逐 id 反频率加权：pw = ((1/n_ids)/freq)^α，clamp 到 [0.25, 8]
            let mut cnt: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
            let mut ids = vec![0u32; hw];
            for p in 0..hw {
                let mut id = 0u32;
                for plane in &s.planes {
                    id = id * 3 + plane[p] as u32;
                }
                ids[p] = id;
                if s.valid[p] == 1 {
                    *cnt.entry(id).or_insert(0) += 1;
                }
            }
            let total: f32 = cnt.values().sum::<usize>() as f32;
            let n_ids = cnt.len().max(1) as f32;
            let mut pw = vec![1.0f32; hw];
            for (p, wp) in pw.iter_mut().enumerate() {
                if s.valid[p] != 1 {
                    continue;
                }
                let f = *cnt.get(&ids[p]).unwrap_or(&1) as f32 / total.max(1.0);
                *wp = ((1.0 / n_ids) / f.max(1e-6))
                    .powf(args.id_weight)
                    .clamp(0.25, 8.0);
            }
            trit_parts::loss::loss_and_grad_weighted(&logits, &s.planes, &s.valid, hw, k, &w, &pw)
        } else if args.bnd_weight > 0.0 {
            // 稀疏类（如实例边界）加权：标签 id == --bnd-label 的像素损失 × bnd_weight
            let mut pw = vec![1.0f32; hw];
            for (p, wp) in pw.iter_mut().enumerate() {
                let mut id = 0u32;
                for plane in &s.planes {
                    id = id * 3 + plane[p] as u32;
                }
                if id + 1 == args.bnd_label {
                    *wp = args.bnd_weight;
                }
            }
            trit_parts::loss::loss_and_grad_weighted(&logits, &s.planes, &s.valid, hw, k, &w, &pw)
        } else {
            loss_and_grad(&logits, &s.planes, &s.valid, hw, k, &w)
        };
        let dw = trit_parts::loss::plane_weights(args.depth_planes.max(1));
        let dout_depth = match (&dep, &tr.depth_out) {
            (Some((p, v)), Some(buf)) => {
                let dlogits = gpu.read(buf, hw * net.depth_cout() as usize);
                let dout = depth_loss_grad(&dlogits, p, v, &dw, args.depth_weight);
                let b = gpu.buf(&dout.grad, "dout_depth");
                dep_out = Some(dout);
                Some(b)
            }
            _ => {
                dep_out = None;
                None
            }
        };
        let dout = gpu.buf(&out.grad, "dout");
        net.backward_full(&gpu, &tr, &dout, dout_depth.as_ref(), res);
        let dout = gpu.buf(&out.grad, "dout");
        net.backward(&gpu, &tr, &dout, res);
        if args.batch {
            gpu.end_batch();
        }
        if (step + 1) % args.accum == 0 {
            // 线性衰减到 --lr-final（缺省恒定）
            let frac = if args.steps > 1 {
                step as f32 / (args.steps - 1) as f32
            } else {
                0.0
            };
            let lr = match args.lr_final {
                Some(end) => args.lr + (end - args.lr) * frac,
                None => args.lr,
            };
            net.update(&gpu, lr, args.ste, args.adam, 1.0 / args.accum as f32);
        }
        best = best.min(out.loss);
        last_loss = out.loss;
        ma = if ma.is_nan() {
            out.loss
        } else {
            0.95 * ma + 0.05 * out.loss
        };
        if args.eval_every > 0 && (step + 1) % args.eval_every == 0 {
            let (scene0, file0) = &paths[0];
            if let Ok(s0) = load_frame_sub(scene0, file0, k, &args.seg_sub) {
                let xb0 = gpu.buf(&s0.rgb, "x");
                let tr0 = net.forward(&gpu, xb0, res);
                let lg0 = gpu.read(&tr0.out, hw * ch);
                let dep0 = if args.depth_planes > 0 {
                    trit_parts::data::depth_planes(
                        scene0,
                        file0,
                        hw,
                        args.depth_planes,
                        args.depth_lo,
                        args.depth_hi,
                    )
                    .ok()
                } else {
                    None
                };
                let ev = loss_and_grad(&lg0, &s0.planes, &s0.valid, hw, k, &w);
                let dacc = match (&dep0, &tr0.depth_out) {
                    (Some((p, v)), Some(buf)) => {
                        let dl = gpu.read(buf, hw * net.depth_cout() as usize);
                        Some(depth_loss_grad(&dl, p, v, &dw, args.depth_weight).acc)
                    }
                    _ => None,
                };
                println!(
                    "  [固定帧] step {}  loss {:.4}  平面 {:?}  valid {:.4}  深度 {:?}",
                    step + 1,
                    ev.loss,
                    ev.plane_acc
                        .iter()
                        .map(|v| (v * 1000.0).round() / 1000.0)
                        .collect::<Vec<_>>(),
                    ev.valid_acc,
                    dacc.map(|a| a
                        .iter()
                        .map(|v| (v * 1000.0).round() / 1000.0)
                        .collect::<Vec<_>>())
                );
            }
        }
        if args.save_every > 0
            && (step + 1) % args.save_every == 0
            && let Some(p) = &save_path
        {
            match trit_parts::state::save(&net, p, resumed_step + (step + 1) as u64, rng) {
                Ok(()) => println!("  [断点] step {} 已存", step + 1),
                Err(e) => eprintln!("  [断点] 保存失败: {e}"),
            }
        }
        if step % args.report == 0 || step + 1 == args.steps {
            let dstr = match &dep_out {
                Some(d) => format!(
                    "  深度平面 {:?}  (l1 {:.4})",
                    d.acc
                        .iter()
                        .map(|v| (v * 1000.0).round() / 1000.0)
                        .collect::<Vec<_>>(),
                    d.loss
                ),
                None => String::new(),
            };
            println!(
                "step {:>5}  loss {:.4} (ma {:.4})  平面(部件像素) {:?}  valid {:.4}{dstr}",
                step + 1,
                out.loss,
                ma,
                out.plane_acc
                    .iter()
                    .map(|v| (v * 1000.0).round() / 1000.0)
                    .collect::<Vec<_>>(),
                out.valid_acc
            );
        }
    }
    println!("最小 loss {best:.4}");

    if let Some(path) = &save_path {
        match trit_parts::state::save(&net, path, resumed_step + args.steps as u64, rng) {
            Ok(()) => println!("断点已存 {}", path.display()),
            Err(e) => {
                eprintln!("断点保存失败: {e}");
                std::process::exit(1);
            }
        }
    }
    if let Some(tpm) = &args.export_tpm {
        let mut m = trit_parts::tpm::from_net(&net);
        if args.tpm_channels > 0 {
            m = m.truncate_head(args.tpm_channels);
        }
        if let Some(parent) = tpm.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).ok();
        }
        m.save(tpm).unwrap_or_else(|e| {
            eprintln!("导出部署包失败: {e}");
            std::process::exit(1);
        });
        let n: usize = m.layers.iter().map(|l| l.q.len()).sum();
        println!(
            "部署包 {}（{} 层，{} 个三值权重）",
            tpm.display(),
            m.layers.len(),
            n
        );
    }
    if let Some(mp) = manifest_path(&args) {
        write_manifest(
            &mp,
            &args,
            &paths,
            save_path.as_ref(),
            resumed_step,
            best,
            last_loss,
        );
    }
}

/// 清单缺省落在部署包旁边（`<tpm>.manifest.json`），目录器按这个位置收。
fn manifest_path(args: &args::Args) -> Option<PathBuf> {
    if let Some(p) = &args.run_manifest {
        return Some(p.clone());
    }
    args.export_tpm.as_ref().map(|t| {
        let mut s = t.as_os_str().to_owned();
        s.push(".manifest.json");
        PathBuf::from(s)
    })
}

fn jnum(v: f32) -> String {
    if v.is_finite() {
        format!("{v}")
    } else {
        "null".into()
    }
}

fn write_manifest(
    path: &PathBuf,
    args: &args::Args,
    frames: &[(PathBuf, PathBuf)],
    state: Option<&PathBuf>,
    resumed_step: u64,
    best: f32,
    last: f32,
) {
    use trit_parts::fingerprint::file_sha256;
    use trit_parts::partsrun::{created_unix, git_out, json_str, norm};

    let fp = match trit_parts::partsrun::fingerprint_train(frames, &args.seg_sub) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("数据指纹失败: {e}");
            std::process::exit(1);
        }
    };
    let commit = git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !git_out(&["status", "--porcelain"]).trim().is_empty();
    let created = created_unix();
    let argv: Vec<String> = std::env::args().skip(1).map(|a| json_str(&a)).collect();
    let roots: Vec<String> = args.data.iter().map(|p| json_str(&norm(p))).collect();
    let fp_json = format!(
        "{{\"dirs\": {}, \"files\": {}, \"bytes\": {}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{}\", \"frames\": {}, \"roots\": [{}], \"limit\": {}}}",
        fp.dirs,
        fp.files,
        fp.bytes,
        fp.hash,
        frames.len(),
        roots.join(", "),
        args.limit
    );
    let blob = |p: &PathBuf| {
        format!(
            "{{\"path\": {}, \"sha256\": \"{}\", \"bytes\": {}}}",
            json_str(&norm(p)),
            file_sha256(p).unwrap_or_default(),
            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
        )
    };
    let model = match state {
        Some(p) if p.exists() => blob(p),
        _ => "null".into(),
    };
    let deploy = match &args.export_tpm {
        Some(p) if p.exists() => {
            let mut b = blob(p);
            b.pop();
            format!("{b}, \"channels\": {}}}", args.tpm_channels)
        }
        _ => "null".into(),
    };
    let lr_final = match args.lr_final {
        Some(v) => jnum(v),
        None => "null".into(),
    };
    let config = format!(
        "{{\"steps\": {}, \"resumed_from\": {resumed_step}, \"lr\": {}, \"lr_final\": {lr_final}, \"accum\": {}, \"res\": {}, \"k\": {}, \"seed\": {}, \"width\": {}, \"seg_sub\": {}, \"bnd_label\": {}, \"bnd_weight\": {}, \"id_weight\": {}, \"depth_planes\": {}, \"residual\": {}, \"adam\": {}, \"batch\": {}}}",
        args.steps,
        jnum(args.lr),
        args.accum,
        args.res,
        args.k,
        args.seed,
        jnum(args.width),
        json_str(&args.seg_sub),
        args.bnd_label,
        jnum(args.bnd_weight),
        jnum(args.id_weight),
        args.depth_planes,
        args.residual,
        args.adam,
        args.batch
    );
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"parts-train\",\n  \"created_unix\": {created},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {config},\n  \"data\": {fp_json},\n  \"model\": {model},\n  \"deploy\": {deploy},\n  \"result\": {{\"final_loss\": {}, \"best_loss\": {}, \"steps\": {}}}\n}}\n",
        json_str(&commit),
        argv.join(", "),
        jnum(last),
        jnum(best),
        resumed_step + args.steps as u64
    );
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, body).unwrap_or_else(|e| {
        eprintln!("写清单失败 {}: {e}", path.display());
        std::process::exit(1);
    });
    println!("清单 {}", path.display());
}
