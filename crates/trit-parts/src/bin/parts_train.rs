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

use trit_learn::Ste;
use trit_parts::data::{Sample, frames, load_frame_sub};
use trit_parts::gpu::Gpu;
use trit_parts::loss::{depth_loss_grad, loss_and_grad, plane_weights};
use trit_parts::net::Net;
use trit_parts::parts::{PartList, evaluate_frame};
use trit_parts::weights::Tensors;

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
    /// 合成数据：光度抖动 + 噪声（开关）。
    augment: bool,
    /// 额外做带标签平移（像素上限，0 = 不做）。
    augment_shift: i32,
    /// 深度头：附加的三值平面数（0 = 关）。
    depth_planes: usize,
    /// 深度项权重。
    depth_weight: f32,
    /// 视差区间（1/米）。
    depth_lo: f32,
    depth_hi: f32,
    /// head 的学习率倍率。
    head_lr: f32,
    /// 冻结除最后一层外的所有层。
    freeze: bool,
    /// 只保留最后 N 层可训练（0 = 不额外冻结）。
    unfreeze_last: usize,
    /// 冻结分割 head。
    freeze_head: bool,
    /// 标签子目录（缺省 `seg`；实例标签用 `inst`）。
    seg_sub: String,
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
        augment: false,
        augment_shift: 0,
        depth_planes: 0,
        depth_weight: 0.5,
        depth_lo: 0.15,
        depth_hi: 0.65,
        head_lr: 1.0,
        freeze: false,
        unfreeze_last: 0,
        freeze_head: false,
        seg_sub: "seg".into(),
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
            "--augment" => a.augment = true,
            "--augment-shift" => {
                a.augment_shift = val()?.parse().map_err(|_| "--augment-shift 非整数")?
            }
            "--ste" => {
                a.ste = match val()?.as_str() {
                    "identity" => Ste::Identity,
                    "clipped" => Ste::Clipped { ratio: 1.0 },
                    "soft" => Ste::Soft,
                    other => return Err(format!("未知 STE {other}")),
                }
            }
            "--depth-planes" => {
                a.depth_planes = val()?.parse().map_err(|_| "--depth-planes 非整数")?
            }
            "--depth-weight" => {
                a.depth_weight = val()?.parse().map_err(|_| "--depth-weight 非数字")?
            }
            "--depth-lo" => a.depth_lo = val()?.parse().map_err(|_| "--depth-lo 非数字")?,
            "--depth-hi" => a.depth_hi = val()?.parse().map_err(|_| "--depth-hi 非数字")?,
            "--head-lr" => a.head_lr = val()?.parse().map_err(|_| "--head-lr 非数字")?,
            "--freeze-backbone" => a.freeze = true,
            "--freeze-head" => a.freeze_head = true,
            "--unfreeze-last" => {
                a.unfreeze_last = val()?.parse().map_err(|_| "--unfreeze-last 非整数")?
            }
            "--seg-sub" => a.seg_sub = val()?,
            "--sgd" => a.adam = false,
            other => return Err(format!("未知参数 {other}")),
        }
        // 纯开关类参数只前进 1，否则会吞掉下一个参数（`--residual --steps 4000` 会把 --steps 吃掉）
        i += match key {
            "--sgd" | "--residual" | "--batch" | "--augment" | "--freeze-backbone"
            | "--freeze-head" => 1,
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

    // 数据：按目录列帧，--limit 截断。`--data` 可以是单场景（含 rgb/），
    // 也可以是「场景的父目录」（其子目录各含 rgb/）。
    let mut paths: Vec<(PathBuf, PathBuf)> = Vec::new();
    for root in &args.data {
        if root.join("rgb").is_dir() {
            for p in frames(root) {
                paths.push((root.clone(), p));
            }
            continue;
        }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(root)
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .map(|e| e.path())
                    .filter(|p| p.join("rgb").is_dir())
                    .collect()
            })
            .unwrap_or_default();
        subs.sort();
        if subs.is_empty() {
            eprintln!("{}: 既不是场景（无 rgb/），也没有含 rgb/ 的子目录", root.display());
        }
        for scene in subs {
            for p in frames(&scene) {
                paths.push((scene.clone(), p));
            }
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
        let out = loss_and_grad(&logits, &s.planes, &s.valid, hw, k, &w);
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
    let ch = net.head_cout() as usize;

    let res = 384u32;
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
