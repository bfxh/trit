//! 训练循环：QAT（三值权重 + STE），全流程 Rust + GPU。
//!
//! 损失口径与 Python 基线一致：层级加权 CE（k 条平面）+ 0.5·BCE（valid），
//! Adam/SGD，支持梯度累加与断点续跑。
//!
//! 例（4 帧过拟合自检）：
//! ```text
//! parts-train --data D:/KF/.yanagent/drift-data/big-r2.4 --init D:/KF/trit-data/parts-parity \
//!   --limit 4 --steps 120 --lr 1e-3 --report 20
//! ```

use std::path::PathBuf;

use trit_parts::data::{Sample, frames, load_frame};
use trit_parts::gpu::Gpu;
use trit_parts::loss::{loss_and_grad, plane_weights};
use trit_parts::net::Net;
use trit_parts::parts::{PartList, evaluate_frame};
use trit_parts::weights::Tensors;
use trit_train::Ste;

struct Args {
    data: Vec<PathBuf>,
    init: Option<PathBuf>,
    resume: Option<PathBuf>,
    save: Option<PathBuf>,
    selfcheck: Option<PathBuf>,
    eval: Option<PathBuf>,
    manifest: Option<PathBuf>,
    steps: usize,
    lr: f32,
    accum: usize,
    ste: Ste,
    adam: bool,
    seed: u64,
    limit: usize,
    report: usize,
    save_every: usize,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        data: Vec::new(),
        init: None,
        resume: None,
        save: None,
        selfcheck: None,
        eval: None,
        manifest: None,
        steps: 100,
        lr: 1e-3,
        accum: 1,
        ste: Ste::Identity,
        adam: true,
        seed: 1,
        limit: 0,
        report: 10,
        save_every: 1000,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let key = argv[i].as_str();
        let val = || {
            argv.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{key} 缺值"))
        };
        match key {
            "--data" => a.data = val()?.split(',').map(PathBuf::from).collect(),
            "--init" => a.init = Some(PathBuf::from(val()?)),
            "--resume" => a.resume = Some(PathBuf::from(val()?)),
            "--save" => a.save = Some(PathBuf::from(val()?)),
            "--selfcheck" => a.selfcheck = Some(PathBuf::from(val()?)),
            "--eval" => a.eval = Some(PathBuf::from(val()?)),
            "--manifest" => a.manifest = Some(PathBuf::from(val()?)),
            "--steps" => a.steps = val()?.parse().map_err(|_| "--steps 非整数")?,
            "--lr" => a.lr = val()?.parse().map_err(|_| "--lr 非数字")?,
            "--accum" => a.accum = val()?.parse().map_err(|_| "--accum 非整数")?,
            "--seed" => a.seed = val()?.parse().map_err(|_| "--seed 非整数")?,
            "--limit" => a.limit = val()?.parse().map_err(|_| "--limit 非整数")?,
            "--report" => a.report = val()?.parse().map_err(|_| "--report 非整数")?,
            "--save-every" => a.save_every = val()?.parse().map_err(|_| "--save-every 非整数")?,
            "--ste" => {
                a.ste = match val()?.as_str() {
                    "identity" => Ste::Identity,
                    "clipped" => Ste::Clipped { ratio: 1.0 },
                    "soft" => Ste::Soft,
                    other => return Err(format!("未知 STE {other}")),
                }
            }
            "--sgd" => a.adam = false,
            other => return Err(format!("未知参数 {other}")),
        }
        i += 2;
    }
    if a.data.is_empty() && a.resume.is_none() && a.selfcheck.is_none() && a.eval.is_none() {
        return Err("需要 --data / --resume / --selfcheck".into());
    }
    Ok(a)
}

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

    // 数据：按目录列帧，--limit 截断
    let mut paths: Vec<(PathBuf, PathBuf)> = Vec::new();
    for scene in &args.data {
        for p in frames(scene) {
            paths.push((scene.clone(), p));
        }
    }
    if args.limit > 0 {
        paths.truncate(args.limit);
    }
    if paths.is_empty() {
        eprintln!("没有可用帧");
        std::process::exit(2);
    }

    let k = 5usize;
    let res = 384u32;
    let hw = (res * res) as usize;
    let ch = k * 3 + 1;

    let mut resumed_step = 0u64;
    let mut net = match &args.resume {
        Some(p) => match Net::load_state(p) {
            Ok((n, step)) => {
                println!("续跑自 {}（step {step}）", p.display());
                resumed_step = step;
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
            None => Net::random(args.seed),
        },
    };
    net.upload_all(&gpu);
    let w = plane_weights(k);
    println!(
        "帧 {}，k={k}，res={res}，lr={}，accum={}，opt={}，ste={:?}",
        paths.len(),
        args.lr,
        args.accum,
        if args.adam { "adam" } else { "sgd" },
        args.ste
    );

    let save_path = args.save.as_ref().map(|dir| {
        if dir.extension().is_some() {
            dir.clone()
        } else {
            dir.join("state.bin")
        }
    });
    if let Some(p) = &save_path
        && let Some(parent) = p.parent()
    {
        std::fs::create_dir_all(parent).ok();
    }
    let mut rng = args.seed | 1;
    let mut best = f32::INFINITY;
    for step in 0..args.steps {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        let (scene, file) = &paths[(rng % paths.len() as u64) as usize];
        let s: Sample = match load_frame(scene, file, k) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("读帧失败: {e}");
                std::process::exit(1);
            }
        };
        let xb = gpu.buf(&s.rgb, "x");
        let tr = net.forward(&gpu, xb, res);
        let logits = gpu.read(&tr.out, hw * ch);
        let out = loss_and_grad(&logits, &s.planes, &s.valid, hw, k, &w);
        let dout = gpu.buf(&out.grad, "dout");
        net.backward(&gpu, &tr, &dout, res);
        if (step + 1) % args.accum == 0 {
            net.update(&gpu, args.lr, args.ste, args.adam, 1.0 / args.accum as f32);
        }
        best = best.min(out.loss);
        if args.save_every > 0
            && (step + 1) % args.save_every == 0
            && let Some(p) = &save_path
        {
            match net.save_state(p, resumed_step + (step + 1) as u64) {
                Ok(()) => println!("  [断点] step {} 已存", step + 1),
                Err(e) => eprintln!("  [断点] 保存失败: {e}"),
            }
        }
        if step % args.report == 0 || step + 1 == args.steps {
            println!(
                "step {:>5}  loss {:.4}  平面(部件像素) {:?}  valid {:.4}",
                step + 1,
                out.loss,
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
        match net.save_state(path, resumed_step + args.steps as u64) {
            Ok(()) => println!("断点已存 {}", path.display()),
            Err(e) => {
                eprintln!("断点保存失败: {e}");
                std::process::exit(1);
            }
        }
    }
}

/// 自检：拿 parity 夹具验证「整网前向 == 推理二进制」的口径（三值 vs PyTorch 三值）。
fn selfcheck(dir: &std::path::Path) {
    let gpu = match Gpu::new() {
        Some(g) => g,
        None => {
            eprintln!("没有可用的 GPU 适配器");
            std::process::exit(2);
        }
    };
    let t = Tensors::load(dir).unwrap_or_else(|e| {
        eprintln!("权重读取失败: {e}");
        std::process::exit(1);
    });
    let mut net = Net::from_tensors(&t).unwrap_or_else(|e| {
        eprintln!("建网失败: {e}");
        std::process::exit(1);
    });
    net.upload_all(&gpu);
    let x = read_f32(&dir.join("input.bin"));
    let res = (x.len() / 3).isqrt() as u32;
    let hw = (res * res) as usize;
    let xb = gpu.buf(&x, "x");
    let tr = net.forward(&gpu, xb, res);
    let out = gpu.read(&tr.out, hw * 16);
    let qt = read_f32(&dir.join("logits_trit_py.bin"));
    let mut agree = 0usize;
    let mut tot = 0usize;
    let mut maxd = 0f32;
    for i in 0..5 {
        for p in 0..hw {
            let (mut ra, mut rb) = (0usize, 0usize);
            let (mut va, mut vb) = (f32::MIN, f32::MIN);
            for c in 0..3 {
                let a = out[p * 16 + i * 3 + c];
                let b = qt[(i * 3 + c) * hw + p];
                maxd = maxd.max((a - b).abs());
                if a > va {
                    va = a;
                    ra = c;
                }
                if b > vb {
                    vb = b;
                    rb = c;
                }
            }
            tot += 1;
            agree += usize::from(ra == rb);
        }
    }
    println!(
        "自检：平面 argmax 一致率 {:.6} ({}/{})，max|Δ| {:.3e}",
        agree as f64 / tot as f64,
        agree,
        tot,
        maxd
    );
    if agree != tot {
        println!("（不一致数 {}）", tot - agree);
    }
}

fn read_f32(path: &std::path::Path) -> Vec<f32> {
    let raw = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", path.display());
        std::process::exit(1);
    });
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

/// 评估：同一模型分别用「无约束 argmax」与「清单约束 argmax」解码，指标并排。
fn eval_only(gpu: &Gpu, args: &Args) {
    let scene = args.eval.as_ref().expect("--eval");
    let manifest_path = args.manifest.as_ref().unwrap_or_else(|| {
        eprintln!("--eval 需要 --manifest <parts.json>");
        std::process::exit(2);
    });
    let m = PartList::load(manifest_path).unwrap_or_else(|e| {
        eprintln!("清单读取失败: {e}");
        std::process::exit(1);
    });
    let k = m.k;
    println!("清单 k={k}，部件 {} 个", m.parts.len());

    let mut net = match &args.resume {
        Some(p) => Net::load_state(p).map(|(n, _)| n),
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

    let mut paths = frames(scene);
    if args.limit > 0 {
        paths.truncate(args.limit);
    }
    let res = 384u32;
    let hw = (res * res) as usize;
    let ch = k * 3 + 1;

    let mut plane_sum = vec![0f64; k];
    let mut exact = [0f64; 2];
    let mut exact_n = [0usize; 2];
    let mut iou = [0f64; 2];
    let mut iou_n = [0usize; 2];
    let mut unk_ids = [0f64; 2];
    let mut unk_px = [0f64; 2];
    let mut unk_px_n = [0f64; 2];

    for (n, file) in paths.iter().enumerate() {
        let s = load_frame(scene, file, k).unwrap_or_else(|e| {
            eprintln!("读帧失败: {e}");
            std::process::exit(1);
        });
        let xb = gpu.buf(&s.rgb, "x");
        let tr = net.forward(gpu, xb, res);
        let logits = gpu.read(&tr.out, hw * ch);
        for (slot, constrained) in [false, true].into_iter().enumerate() {
            let fm = evaluate_frame(&logits, &s.planes, &s.valid, &m, constrained, true);
            for (acc, v) in fm.plane_acc.iter().enumerate() {
                if !constrained {
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
        if std::env::var("TRIT_EVAL_DEBUG").is_ok() && n == 0 {
            let dbg = evaluate_frame(&logits, &s.planes, &s.valid, &m, false, true);
            let vp = logits.chunks_exact(ch).filter(|c| c[k * 3] > 0.0).count();
            let gt_px = s.valid.iter().filter(|v| **v == 1).count();
            let mut gt = std::collections::BTreeSet::new();
            for p in 0..hw {
                if s.valid[p] == 1 {
                    let d: Vec<u8> = (0..k).map(|i| s.planes[i][p]).collect();
                    gt.insert(m.id_of(&d));
                }
            }
            println!(
                "  [dbg] 帧0: GT 部件像素 {gt_px}, 预测 valid 像素 {vp}, GT 不同 id {}, id_exact {:?}, iou {} 项, 未知 {:?}/{:?}",
                gt.len(),
                dbg.id_exact,
                dbg.part_iou.len(),
                dbg.unknown_ids,
                dbg.unknown_pixels
            );
        }
        if (n + 1) % 10 == 0 || n + 1 == paths.len() {
            println!("  已评估 {}/{}", n + 1, paths.len());
        }
    }
    let nf = paths.len() as f64;
    let planes: Vec<f64> = plane_sum.iter().map(|v| v / nf).collect();
    let mean = |s: f64, n: usize| if n > 0 { s / n as f64 } else { 0.0 };
    println!("帧数 {}", paths.len());
    let plane_show: Vec<f64> = planes
        .iter()
        .map(|v| (v * 1000.0).round() / 1000.0)
        .collect();
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
