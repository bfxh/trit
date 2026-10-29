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
    /// 进程存档目录：存在则自动续跑，运行中按 save-every 原子落盘。
    state: Option<PathBuf>,
    selfcheck: Option<PathBuf>,
    eval: Option<PathBuf>,
    manifest: Option<PathBuf>,
    steps: usize,
    lr: f32,
    /// 终值学习率（线性衰减）；缺省等于 lr。
    lr_final: Option<f32>,
    accum: usize,
    ste: Ste,
    adam: bool,
    seed: u64,
    limit: usize,
    report: usize,
    save_every: usize,
    eval_every: usize,
    /// 从零训练时的通道宽度倍数（1.0 = 32/64/128；1.5 = 48/96/192）。
    width: f32,
    /// net2net：从这份断点把通道搬进按 `--width` 放大的新网络（再精调）。
    expand_from: Option<PathBuf>,
    /// 块内残差（cin==cout 的卷积层）。
    residual: bool,
    /// 可拆卸加速层（opt-in）：把单样本内的所有 dispatch 收进一个 encoder 再提交。
    /// 数学完全不变；实测在 GPU 被占用时无加速，默认关，GPU 空闲时再评估。
    batch: bool,
}

fn parse() -> Result<Args, String> {
    let mut a = Args {
        data: Vec::new(),
        init: None,
        resume: None,
        save: None,
        state: None,
        selfcheck: None,
        eval: None,
        manifest: None,
        steps: 100,
        lr: 1e-3,
        lr_final: None,
        accum: 1,
        ste: Ste::Identity,
        adam: true,
        seed: 1,
        limit: 0,
        report: 10,
        save_every: 1000,
        eval_every: 0,
        width: 1.0,
        expand_from: None,
        residual: false,
        batch: false,
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
            "--state" => a.state = Some(PathBuf::from(val()?)),
            "--selfcheck" => a.selfcheck = Some(PathBuf::from(val()?)),
            "--eval" => a.eval = Some(PathBuf::from(val()?)),
            "--manifest" => a.manifest = Some(PathBuf::from(val()?)),
            "--steps" => a.steps = val()?.parse().map_err(|_| "--steps 非整数")?,
            "--lr" => a.lr = val()?.parse().map_err(|_| "--lr 非数字")?,
            "--lr-final" => a.lr_final = Some(val()?.parse().map_err(|_| "--lr-final 非数字")?),
            "--accum" => a.accum = val()?.parse().map_err(|_| "--accum 非整数")?,
            "--seed" => a.seed = val()?.parse().map_err(|_| "--seed 非整数")?,
            "--limit" => a.limit = val()?.parse().map_err(|_| "--limit 非整数")?,
            "--report" => a.report = val()?.parse().map_err(|_| "--report 非整数")?,
            "--save-every" => a.save_every = val()?.parse().map_err(|_| "--save-every 非整数")?,
            "--eval-every" => a.eval_every = val()?.parse().map_err(|_| "--eval-every 非整数")?,
            "--width" => a.width = val()?.parse().map_err(|_| "--width 非数字")?,
            "--expand-from" => a.expand_from = Some(PathBuf::from(val()?)),
            "--residual" => a.residual = true,
            "--batch" => a.batch = true,
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
        // 纯开关类参数只前进 1，否则会吞掉下一个参数（`--residual --steps 4000` 会把 --steps 吃掉）
        i += match key {
            "--sgd" | "--residual" | "--batch" => 1,
            _ => 2,
        };
    }
    if a.data.is_empty()
        && a.resume.is_none()
        && a.state.is_none()
        && a.selfcheck.is_none()
        && a.eval.is_none()
    {
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
    if args.residual {
        net.set_residual(true);
        println!("块内残差：开（e1b/e2b/e3b/d2b/d1b）");
    }
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

    if let Some(p) = &save_path
        && let Some(parent) = p.parent()
    {
        std::fs::create_dir_all(parent).ok();
    }
    let mut rng = resumed_rng.unwrap_or(args.seed | 1);
    let mut best = f32::INFINITY;
    let mut ma = f32::NAN; // 损失的指数移动平均（单帧噪声大，直接打最后一帧会误导）
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
        if args.batch {
            gpu.begin_batch();
        }
        let tr = net.forward(&gpu, xb, res);
        let logits = gpu.read(&tr.out, hw * ch);
        let out = loss_and_grad(&logits, &s.planes, &s.valid, hw, k, &w);
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
        ma = if ma.is_nan() {
            out.loss
        } else {
            0.95 * ma + 0.05 * out.loss
        };
        if args.eval_every > 0 && (step + 1) % args.eval_every == 0 {
            let (scene0, file0) = &paths[0];
            if let Ok(s0) = load_frame(scene0, file0, k) {
                let xb0 = gpu.buf(&s0.rgb, "x");
                let tr0 = net.forward(&gpu, xb0, res);
                let lg0 = gpu.read(&tr0.out, hw * ch);
                let ev = loss_and_grad(&lg0, &s0.planes, &s0.valid, hw, k, &w);
                println!(
                    "  [固定帧] step {}  loss {:.4}  平面 {:?}  valid {:.4}",
                    step + 1,
                    ev.loss,
                    ev.plane_acc
                        .iter()
                        .map(|v| (v * 1000.0).round() / 1000.0)
                        .collect::<Vec<_>>(),
                    ev.valid_acc
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
            println!(
                "step {:>5}  loss {:.4} (ma {:.4})  平面(部件像素) {:?}  valid {:.4}",
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
    let scenes: Vec<PathBuf> = args
        .eval
        .as_ref()
        .expect("--eval")
        .to_string_lossy()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .collect();
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

    let res = 384u32;
    let hw = (res * res) as usize;
    let ch = k * 3 + 1;

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

    for scene in &scenes {
        let mut paths = frames(scene);
        let cap = if args.limit > 0 {
            args.limit
        } else {
            usize::MAX
        };
        paths.truncate(cap);
        for (n, file) in paths.iter().enumerate() {
            let s = load_frame(scene, file, k).unwrap_or_else(|e| {
                eprintln!("读帧失败: {e}");
                std::process::exit(1);
            });
            let xb = gpu.buf(&s.rgb, "x");
            let tr = net.forward(gpu, xb, res);
            let logits = gpu.read(&tr.out, hw * ch);
            frames_n += 1;
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
