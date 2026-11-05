use std::path::PathBuf;

use trit_parts::diffuse::{Denoiser, PatchMlp, Schedule};
use trit_parts::tconv::Net;

use super::cli::{arg, fail};
use super::look::fit_rgb;
use super::texgen::Den;

pub(crate) fn tex_gen_cmd(argv: &[String]) {
    use trit_parts::tex::{seam_ratio, tile};

    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200000);
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let steps: usize = arg(argv, "--steps-t")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let eval_seed: u64 = arg(argv, "--eval-seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    // 0.003 是实测能收敛的档；0.01 在真实材质上会从 loss 0.62 涨到 6.48（GENERAL-PLAN §6.21）
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.003);
    let hid: usize = arg(argv, "--hid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(32);
    let outdir = arg(argv, "--out").map(PathBuf::from);
    let data_dir = arg(argv, "--data").map(PathBuf::from);
    let patch: usize = arg(argv, "--patch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    // --normalize：按图 z-score 到单位方差（扩散噪声表按单位方差设计；也顺带得到显式亮度/对比控制）
    let normalize = argv.iter().any(|a| a == "--normalize");

    let kinds = ["wood", "marble", "rust", "grid"];
    let gray = |kind: &str, sd: u64| -> Vec<f32> {
        tile(kind, n, sd)
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    };
    let sched = Schedule::cosine(steps);
    let abar = sched.abar();
    // 数据源：--data <dir> 用真实材质（每张图一个样本，无条件）；否则程序化材质
    let gray_img = |f: &PathBuf| -> Vec<f32> {
        let (w0, h0, pix) = trit_parts::data::read_rgb(f).unwrap_or_else(|e| fail(&e));
        let rgb = fit_rgb(w0, h0, &pix, n);
        rgb.as_chunks::<3>()
            .0
            .iter()
            .map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
            .collect()
    };
    // 评测图必须钉死：`--holdout-names` 按文件名前缀匹配（扩数据集时排序会变，`take(2)` 会漂）
    let hold_names: Vec<String> = arg(argv, "--holdout-names")
        .map(|v| {
            v.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut img_files: Vec<PathBuf> = Vec::new();
    let images: Vec<Vec<f32>> = match &data_dir {
        Some(dir) => {
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", dir.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect();
            files.sort();
            if files.len() < 3 {
                fail("--data 至少 3 张 PNG");
            }
            files.retain(|f| {
                let stem = f
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default();
                !hold_names.iter().any(|h| stem.starts_with(h.as_str()))
            });
            if !hold_names.is_empty() && files.len() < 3 {
                fail("--holdout-names 之后训练集不足 3 张");
            }
            img_files = files.clone();
            files.iter().map(gray_img).collect()
        }
        None => kinds.iter().map(|k| gray(k, seed)).collect(),
    };
    // 评测集：真实材质时用留出名单里的那几张
    let eval_real: Vec<(String, Vec<f32>)> = match &data_dir {
        Some(dir) if !hold_names.is_empty() => {
            let mut v = Vec::new();
            for f in std::fs::read_dir(dir)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", dir.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
            {
                let stem = f
                    .file_stem()
                    .map(|x| x.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if hold_names.iter().any(|h| stem.starts_with(h.as_str())) {
                    v.push((stem.clone(), f.clone()));
                }
            }
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v.into_iter()
                .map(|(name, f)| (name, gray_img(&f)))
                .collect()
        }
        _ => Vec::new(),
    };
    // 训练集均值是有用的校准读数（采样塌成常数时先看它是不是偏了）
    let data_mean = images
        .iter()
        .map(|g| g.iter().sum::<f32>() / g.len() as f32)
        .sum::<f32>()
        / images.len() as f32;
    println!("训练图 {} 张，均值 {data_mean:.3}", images.len());
    // S6.3：材质类别（--labels <tsv: assetId\tclass>）→ 每张图的类索引
    let mut film_k: usize = arg(argv, "--film")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut img_class: Vec<Option<usize>> = vec![None; images.len()];
    if film_k > 0 {
        let labels_path = arg(argv, "--labels")
            .unwrap_or_else(|| fail("--film 需要 --labels <tsv>（assetId\\tclass）"));
        let txt = std::fs::read_to_string(&labels_path)
            .unwrap_or_else(|e| fail(&format!("读不了 {labels_path}: {e}")));
        let mut names: Vec<String> = Vec::new();
        let mut ids: Vec<String> = Vec::new();
        for line in txt.lines() {
            let mut it = line.split('\t');
            let (Some(aid), Some(cls)) = (it.next(), it.next()) else {
                continue;
            };
            let name = cls.trim();
            let idx = match names.iter().position(|n| n == name) {
                Some(i) => i,
                None => {
                    names.push(name.to_string());
                    names.len() - 1
                }
            };
            ids.push(format!("{aid}\t{idx}"));
        }
        if names.len() != film_k {
            fail(&format!(
                "--film {film_k} 与标签里的类数 {} 不一致：{:?}",
                names.len(),
                names
            ));
        }
        for (i, f) in img_files.iter().enumerate() {
            let stem = f
                .file_stem()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_default();
            for row in &ids {
                let (aid, idx) = row.split_once('\t').unwrap();
                if stem.starts_with(aid) {
                    img_class[i] = idx.parse().ok();
                    break;
                }
            }
        }
        let hit = img_class.iter().filter(|c| c.is_some()).count();
        println!(
            "类别条件：{film_k} 类，命中 {hit}/{} 张；类名 {:?}",
            images.len(),
            names
        );
    }

    // --classes a,b：只保留这些类并重映射到 0..n（S6.3 的小规模对照）
    let sel: Vec<usize> = arg(argv, "--classes")
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    let images: Vec<Vec<f32>> = if sel.len() >= 2 && film_k > 0 {
        let keep: Vec<usize> = (0..images.len())
            .filter(|&i| img_class[i].is_some_and(|c| sel.contains(&c)))
            .collect();
        if keep.len() < 40 {
            fail(&format!("--classes {sel:?} 只有 {} 张，太少", keep.len()));
        }
        img_class = keep
            .iter()
            .map(|&i| img_class[i].map(|c| sel.iter().position(|&s| s == c).unwrap()))
            .collect();
        film_k = sel.len();
        println!("只训类 {sel:?}：{} 张（重映射 0..{}）", keep.len(), film_k);
        keep.iter().map(|&i| images[i].clone()).collect()
    } else {
        images
    };

    // 归一化：每张图减去自身均值、除以自身标准差（保留反归一化参数）
    let img_norm: Vec<(f32, f32)> = images
        .iter()
        .map(|g| {
            let m = g.iter().sum::<f32>() / g.len() as f32;
            let v = g.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / g.len() as f32;
            (m, v.sqrt().max(1e-4))
        })
        .collect();
    let images: Vec<Vec<f32>> = if normalize {
        images
            .iter()
            .zip(&img_norm)
            .map(|(g, (m, s))| g.iter().map(|x| (x - m) / s).collect())
            .collect()
    } else {
        images
    };
    let conds = if data_dir.is_some() { 0 } else { kinds.len() };
    // 去噪器：默认 3×3/7×7 补丁 MLP；`--conv 8,8` 换成三值卷积栈（输入 = [x_t, t/T] 两张图）
    let conv_hidden: Vec<usize> = arg(argv, "--conv")
        .map(|v| v.split(',').filter_map(|x| x.trim().parse().ok()).collect())
        .unwrap_or_default();
    let use_bn = argv.iter().any(|a| a == "--bn");
    let use_adam = argv.iter().any(|a| a == "--adam");
    // ε-预测：目标是噪声而不是 x0；线性输出 + 平方误差（x0-预测会把细纹理平均掉）
    let v_mode = argv.iter().any(|a| a == "--v");
    // --cond-stats：把「这张图的均值 + 对比(p95−p5)」作为两路常数条件图，治外观单模
    let cond_stats = argv.iter().any(|a| a == "--cond-stats");
    // --normalize：按图 z-score 到单位方差（扩散噪声表是按单位方差设计的；也顺带得到显式的亮度/对比控制）
    let eps_mode = argv.iter().any(|a| a == "--eps") && !v_mode;
    let linear_mode = eps_mode || v_mode;
    let batch: usize = arg(argv, "--batch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(4);
    let save_path = arg(argv, "--save").map(PathBuf::from);
    let load_path = arg(argv, "--load").map(PathBuf::from);
    // 没有 --conv 时走的是 MLP 分支，那里根本不看 --save/--load：
    // 曾经 `--load` 被静默忽略，采到的是未训练 MLP 的常数图（2026-11-01 实测三份不同 checkpoint 输出逐位相同）。
    if conv_hidden.is_empty() && (load_path.is_some() || save_path.is_some()) {
        fail("--load/--save 只对卷积去噪器生效：请同时给 --conv（隐藏宽度，如 --conv 8,8）");
    }
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut next_u = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let mut loss = 0.0f64;
    let den: Den = if conv_hidden.is_empty() {
        let mut d = PatchMlp::new(patch, conds, hid, seed);
        for step in 0..iters {
            let ki = (step * 7919) % images.len();
            let g = images[ki].clone();
            loss += d.train_shard(&g, &abar, None, n, n, lr, 64, &mut rng) as f64;
            if (step + 1) % 2000 == 0 {
                println!("  step {} loss {:.4}", step + 1, loss / 2000.0);
                loss = 0.0;
            }
        }
        Den::M(d)
    } else {
        let mut net = match &load_path {
            Some(p) => Net::load(p).unwrap_or_else(|e| fail(&e)),
            None => Net::new(2 + 2 * usize::from(cond_stats), &conv_hidden, seed),
        };
        if let Some(d) = arg(argv, "--dil").and_then(|v| v.parse::<usize>().ok()) {
            let file_dil = net.layers.first().map(|l| l.dil).unwrap_or(1);
            if load_path.is_some() && d.max(1) != file_dil {
                fail(&format!(
                    "--dil {d} 与 checkpoint 里的膨胀率 {file_dil} 不一致（改它等于换模型）"
                ));
            }
            net.set_dilation(d);
        }
        net.set_bn(use_bn && load_path.is_none());
        // 可平铺纹理用循环填充：零填充会把边界抹平（接缝比 0.60）
        if argv.iter().any(|a| a == "--circ") {
            net.set_circular(true);
        }
        // 高通匹配权重（对抗 MSE 的条件均值化；0 = 关）。
        // 只有显式给了才覆盖：否则加载 TPC4+ checkpoint 时会把文件里的 highpass 静默抹成 0。
        if let Some(v) = arg(argv, "--hf-weight").and_then(|v| v.parse().ok()) {
            net.highpass = v;
        }
        if linear_mode && load_path.is_none() {
            net.linear_out = true;
        }
        if film_k > 0 && load_path.is_none() {
            net.set_film(film_k);
            net.film_lr_mul = arg(argv, "--film-lr")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0f32);
            println!("FiLM lr 倍数 {}", net.film_lr_mul);
            // coherence 匹配（归一化矩，直击各向异性）
            if let (Some(c), Some(cs), Some(ts), Some(lam)) = (
                arg(argv, "--coh-class").and_then(|v| v.parse::<usize>().ok()),
                arg(argv, "--coh-target").and_then(|v| v.parse::<f32>().ok()),
                arg(argv, "--coh-energy").and_then(|v| v.parse::<f32>().ok()),
                arg(argv, "--coh-loss").and_then(|v| v.parse::<f32>().ok()),
            ) {
                let w_t = arg(argv, "--coh-wt")
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.1f32);
                net.coh = Some((c, cs, ts, lam, w_t));
                net.abar = abar.clone();
                println!("coherence 匹配：类 {c}，c*={cs}，T*={ts}，λ={lam}，能量权 {w_t}");
            }
            // 结构张量矩匹配（S6.3 方向性结构）
            if let (Some(c), Some(t), Some(lam)) = (
                arg(argv, "--stm-class").and_then(|v| v.parse::<usize>().ok()),
                arg(argv, "--stm-target"),
                arg(argv, "--stm-loss").and_then(|v| v.parse::<f32>().ok()),
            ) {
                let parts: Vec<f32> = t.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if parts.len() == 3 {
                    net.stm = Some((c, [parts[0], parts[1], parts[2]], lam));
                    net.abar = abar.clone();
                    println!("结构张量匹配：类 {c}，目标 {:?}，λ={lam}", parts);
                }
            }
            // 方向性损失：只对指定类（如木纹）抑制垂直方向梯度
            if let (Some(c), Some(deg), Some(lam)) = (
                arg(argv, "--dir-class").and_then(|v| v.parse::<usize>().ok()),
                arg(argv, "--dir-angle").and_then(|v| v.parse::<f32>().ok()),
                arg(argv, "--dir-loss").and_then(|v| v.parse::<f32>().ok()),
            ) {
                net.dir = Some((c, deg.to_radians(), lam));
                net.abar = abar.clone();
                println!("方向性损失：类 {c}，目标梯度角 {deg}°，λ={lam}");
            }
            // 每类一个输出头（共享主干）：把「逐类上界」的机制塞回单模型
            if argv.iter().any(|a| a == "--heads") {
                net.set_heads(film_k, seed);
                println!("输出头：{film_k} 个（共享主干）");
            }
            // 辅助分类损失：逼输出携带类别信息（FiLM 单独用会被逐点回归忽略）
            net.aux_w = arg(argv, "--aux")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0f32);
            if net.aux_w > 0.0 {
                net.set_aux(film_k);
                println!("辅助分类损失 λ={}（{} 类）", net.aux_w, film_k);
            }
        }
        let tmax = sched.steps().max(1);
        // 每张图的 (均值, 对比) —— 条件通道的取值
        let img_stats: Vec<(f32, f32)> = images
            .iter()
            .map(|g| {
                let mean = g.iter().sum::<f32>() / g.len() as f32;
                let mut v = g.clone();
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let p = |q: f32| v[((v.len() - 1) as f32 * q).round() as usize];
                (mean, p(0.95) - p(0.05))
            })
            .collect();
        for step in 0..iters {
            let mut items: Vec<trit_parts::tconv::Sample> = Vec::with_capacity(batch);
            for _ in 0..batch {
                let ki = (next_u() * images.len() as f32) as usize % images.len();
                let t = 1 + (next_u() * tmax as f32) as usize % tmax;
                let noise: Vec<f32> = (0..n * n)
                    .map(|_| {
                        let u1 = next_u().max(1e-6);
                        let u2 = next_u();
                        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                    })
                    .collect();
                let x0 = &images[ki];
                let xt = trit_parts::diffuse::add_noise(x0, &noise, abar[t]);
                let tf = t as f32 / tmax as f32;
                let mut maps = vec![xt, vec![tf; n * n]];
                if cond_stats {
                    let (m, c) = img_stats[ki];
                    maps.push(vec![m; n * n]);
                    maps.push(vec![c; n * n]);
                }
                // 10% 条件丢弃（学会空类，避免只能靠类别提示）
                let cls = if film_k > 0 && next_u() < 0.1 {
                    None
                } else {
                    img_class[ki]
                };
                let target = if eps_mode {
                    noise
                } else if v_mode {
                    // v = √ᾱ·ε − √(1−ᾱ)·x0
                    let (sa, se) = (abar[t].sqrt(), (1.0 - abar[t]).sqrt());
                    noise
                        .iter()
                        .zip(x0.iter())
                        .map(|(e, c)| sa * e - se * c)
                        .collect()
                } else {
                    x0.clone()
                };
                items.push((maps, target, cls));
            }
            loss += net.train_batch_xc(&items, n, n, lr, use_adam) as f64;
            if (step + 1) % 2000 == 0 {
                println!(
                    "  step {} loss {:.4} | aux CE {:.3} acc {:.2}",
                    step + 1,
                    loss / 2000.0,
                    net.aux_ce,
                    net.aux_acc
                );
                loss = 0.0;
            }
            // 断点：每 1000 步存一次（本机常被关机，别让整轮白跑）
            if let Some(p) = &save_path
                && (step + 1) % 1000 == 0
            {
                net.save(p).unwrap_or_else(|e| fail(&e));
                println!("  ckpt {} @ step {}", p.display(), step + 1);
            }
        }
        // --diag：线性输出（ε/v）时，判预测器对输入 x_t 的依赖强度。
        // 只换噪声、同 t、同 x0：若 v̂/ε̂ 几乎不变，说明输入被 STE 分流掉了（种子不变性的根因）。
        if argv.iter().any(|a| a == "--diag") && linear_mode {
            let mut r = 0x5EED_1234u64;
            let mut g = |k: usize| -> Vec<f32> {
                (0..k)
                    .map(|_| {
                        r ^= r << 13;
                        r ^= r >> 7;
                        r ^= r << 17;
                        let u1 = ((r >> 11) as f32 / (1u64 << 53) as f32).max(1e-6);
                        r ^= r << 13;
                        r ^= r >> 7;
                        r ^= r << 17;
                        let u2 = (r >> 11) as f32 / (1u64 << 53) as f32;
                        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                    })
                    .collect()
            };
            let corr = |a: &[f32], b: &[f32]| -> f32 {
                let n = a.len() as f32;
                let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
                let (mut sab, mut sa, mut sb) = (0.0f32, 0.0f32, 0.0f32);
                for (x, y) in a.iter().zip(b) {
                    let (dx, dy) = (x - ma, y - mb);
                    sab += dx * dy;
                    sa += dx * dx;
                    sb += dy * dy;
                }
                sab / (sa * sb).sqrt().max(1e-9)
            };
            let rms = |v: &[f32]| -> f32 {
                (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
            };
            let x0 = &images[0];
            for tf in [0.2f32, 0.5, 0.8, 1.0] {
                let t = ((tf * tmax as f32).round() as usize).clamp(1, tmax);
                let e1 = g(n * n);
                let e2 = g(n * n);
                let xt1 = trit_parts::diffuse::add_noise(x0, &e1, abar[t]);
                let xt2 = trit_parts::diffuse::add_noise(x0, &e2, abar[t]);
                let h1 = net.forward_x(&[xt1.clone(), vec![tf; n * n]], n, n);
                let h2 = net.forward_x(&[xt2, vec![tf; n * n]], n, n);
                // 目标：ε 或 v
                let (sa, se) = (abar[t].sqrt(), (1.0 - abar[t]).sqrt());
                let target: Vec<f32> = if v_mode {
                    e1.iter()
                        .zip(x0.iter())
                        .map(|(e, c)| sa * e - se * c)
                        .collect()
                } else {
                    e1.clone()
                };
                let d: Vec<f32> = h1.iter().zip(&h2).map(|(a, b)| a - b).collect();
                let dt: Vec<f32> = h1.iter().zip(&target).map(|(a, b)| a - b).collect();
                println!(
                    "  依赖 t={tf:.1}: corr(ĥ,目标)={:.3} 残差 {:.3} | 换噪声后相对变化 **{:.1}%** | corr(ĥ,x_t)={:.3}",
                    corr(&h1, &target),
                    rms(&dt),
                    100.0 * rms(&d) / rms(&h1).max(1e-6),
                    corr(&h1, &xt1)
                );
            }
        }
        if let Some(p) = &save_path {
            net.save(p).unwrap_or_else(|e| fail(&e));
            println!("权重已存 {}", p.display());
        }
        if v_mode {
            let extra = if cond_stats {
                let want_mean: f32 = arg(argv, "--want-mean")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| {
                        img_stats.iter().map(|s| s.0).sum::<f32>() / img_stats.len() as f32
                    });
                let want_contrast: f32 = arg(argv, "--want-contrast")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| {
                        let mut c: Vec<f32> = img_stats.iter().map(|s| s.1).collect();
                        c.sort_by(|a, b| a.partial_cmp(b).unwrap());
                        c[c.len() / 2]
                    });
                println!("条件: 目标均值 {want_mean:.3}、目标对比 {want_contrast:.3}");
                vec![want_mean, want_contrast]
            } else {
                Vec::new()
            };
            let want_class: Option<usize> = arg(argv, "--want-class")
                .and_then(|v| v.parse().ok())
                .filter(|&k| k < film_k);
            if film_k > 0 {
                println!(
                    "采样类别：{}",
                    want_class
                        .map(|k| k.to_string())
                        .unwrap_or_else(|| "空类".into())
                );
            }
            Den::V(trit_parts::tconv::VDen {
                net,
                abar: abar.clone(),
                extra,
                clamp: !normalize,
                class: want_class,
            })
        } else if eps_mode {
            Den::E(trit_parts::tconv::EpsDen {
                net,
                abar: abar.clone(),
                eps_scale: arg(argv, "--eps-scale")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1.0f32),
            })
        } else {
            Den::C(net)
        }
    };

    // 诊断：在 t/T = 0.2/0.5/0.8 上加噪，看 x0 预测的 MAE（应随时间步增大而变大）
    {
        let g = match (&data_dir, eval_real.first()) {
            (Some(_), Some((_, im))) => im.clone(),
            _ => gray(kinds[0], eval_seed),
        };
        let mut rng2: u64 = 999;
        let mut next = || {
            rng2 ^= rng2 << 13;
            rng2 ^= rng2 >> 7;
            rng2 ^= rng2 << 17;
            (rng2 >> 11) as f32 / (1u64 << 53) as f32
        };
        for &tf in &[0.2f32, 0.5, 0.8] {
            let t = ((tf * (abar.len() - 1) as f32) as usize).min(abar.len() - 1);
            let eps: Vec<f32> = (0..n * n)
                .map(|_| {
                    let u1 = next().max(1e-6);
                    let u2 = next();
                    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
                })
                .collect();
            let xt = trit_parts::diffuse::add_noise(&g, &eps, abar[t]);
            let pred = den.predict_x0(&xt, tf, if conds == 0 { None } else { Some(0) }, n, n);
            let mae = pred.iter().zip(&g).map(|(a, b)| (a - b).abs()).sum::<f32>() / (n * n) as f32;
            let pmid = pred.iter().sum::<f32>() / pred.len() as f32;
            println!(
                "  诊断 t={tf:.1}: 预测 MAE {mae:.3}，预测均值 {pmid:.3}（真值均值 {:.3}）",
                g.iter().sum::<f32>() / g.len() as f32
            );
        }
    }

    let stat = |v: &[f32]| -> (f32, f32) {
        let m = v.iter().sum::<f32>() / v.len() as f32;
        let var = v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32;
        (m, var.sqrt())
    };
    let patch_dist = |img: &[f32], data: &[f32]| -> f32 {
        // 每 8 像素取一个 patch² 补丁，报生成补丁到真实补丁的最小 L2 均值
        let stride = 8usize;
        let half = patch / 2;
        let grab = |src: &[f32], x: usize, y: usize| -> Vec<f32> {
            let mut p = Vec::with_capacity(patch * patch);
            for dy in 0..patch {
                for dx in 0..patch {
                    p.push(src[(y + dy - half) * n + x + dx - half]);
                }
            }
            p
        };
        let mut refs: Vec<Vec<f32>> = Vec::new();
        for y in (half..n - half).step_by(stride) {
            for x in (half..n - half).step_by(stride) {
                refs.push(grab(data, x, y));
            }
        }
        let mut sum = 0.0f32;
        let mut cnt = 0usize;
        for y in (half..n - half).step_by(stride) {
            for x in (half..n - half).step_by(stride) {
                let p = grab(img, x, y);
                let mut best = f32::INFINITY;
                for r in &refs {
                    let d2: f32 = p.iter().zip(r).map(|(a, b)| (a - b) * (a - b)).sum();
                    best = best.min(d2);
                }
                sum += best.sqrt();
                cnt += 1;
            }
        }
        sum / cnt.max(1) as f32
    };

    println!("材质     生成(均值±std)      真实(均值±std)     接缝比  最近补丁距（patch {patch}）");
    let eval_items: Vec<(String, Vec<f32>)> = match &data_dir {
        Some(_) if !eval_real.is_empty() => eval_real.clone(),
        Some(_) => images
            .iter()
            .take(2)
            .enumerate()
            .map(|(i, g)| (format!("data{i}"), g.clone()))
            .collect(),
        None => kinds
            .iter()
            .enumerate()
            .map(|(ki, k)| ((*k).to_string(), gray(k, eval_seed + ki as u64)))
            .collect(),
    };
    for (ki, (k, data)) in eval_items.iter().enumerate() {
        let data = data.clone();
        let (dm, ds) = stat(&data);
        let sample = den.sample(
            &sched,
            n,
            n,
            eval_seed * 131 + ki as u64 + 7,
            if data_dir.is_some() { None } else { Some(ki) },
            argv.iter().any(|a| a == "--eps-space"),
            !normalize,
        );
        let sample: Vec<f32> = if normalize {
            let want_mean: f32 = arg(argv, "--want-mean")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    let mut m: Vec<f32> = img_norm.iter().map(|s| s.0).collect();
                    m.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    m[m.len() / 2]
                });
            let want_std: f32 = arg(argv, "--want-std")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    let mut s: Vec<f32> = img_norm.iter().map(|s| s.1).collect();
                    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    s[s.len() / 2]
                });
            println!("反归一化: 目标均值 {want_mean:.3}、目标标准差 {want_std:.3}");
            sample.iter().map(|x| x * want_std + want_mean).collect()
        } else {
            sample
        };
        let (gm, gs) = stat(&sample);
        let sr = seam_ratio(
            &sample
                .iter()
                .map(|v| [(v * 255.0) as u8; 3])
                .collect::<Vec<_>>(),
            n,
        );
        let pd = patch_dist(&sample, &data);
        println!("{k:<8} {gm:.3}±{gs:.3}      {dm:.3}±{ds:.3}      {sr:.2}    {pd:.3}");
        let _ = &data;
        if let Some(dir) = &outdir {
            std::fs::create_dir_all(dir).ok();
            let mut bytes = format!(
                "P6
{n} {n}
255
"
            )
            .into_bytes();
            for v in &sample {
                let c = (v.clamp(0.0, 1.0) * 255.0) as u8;
                bytes.extend_from_slice(&[c, c, c]);
            }
            std::fs::write(dir.join(format!("gen_{k}.ppm")), bytes).ok();
        }
    }
    println!("（接缝比 ≈1 表示边界与内部同量级；最近补丁距太小 = 在抄训练集）");
}
