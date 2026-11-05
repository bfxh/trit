use std::path::PathBuf;

use trit_parts::diffuse::{Denoiser, PatchMlp, Schedule};
use trit_parts::tconv::Net;

use super::cli::{arg, fail, usage};
use super::look::{avg, fit_rgb};

/// `tex --kind wood|marble|rust|grid --size 256 --out <ppm>`：程序化可平铺材质 + 无缝度。
pub(crate) fn tex_cmd(argv: &[String]) {
    let kind = arg(argv, "--kind").unwrap_or_else(|| "wood".into());
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));
    let px = trit_parts::tex::tile(&kind, n, seed);
    let r = trit_parts::tex::seam_ratio(&px, n);
    let mut bytes = format!(
        "P6
{n} {n}
255
"
    )
    .into_bytes();
    for p in &px {
        bytes.extend_from_slice(p);
    }
    std::fs::write(&out, bytes).unwrap_or_else(|e| fail(&format!("{}: {e}", out.display())));
    println!(
        "{kind}: {n}×{n} 接缝比 {r:.2}（≈1 无缝）→ {}",
        out.display()
    );
}

/// `tex-prior [--patch 3|7] [--cond]`：N5 学习地基——用程序化材质当数据训三值小算子。
/// - 无 `--cond`：按像素回归（局部去噪），`nin = patch²`；
/// - 加 `--cond`：输入拼上材质 one-hot（4 维），验证「条件能否被学到」：
///   评测时同时报「正确条件」与「错误条件」的 PSNR，差得越多说明条件用得越实。
pub(crate) fn tex_prior_cmd(argv: &[String]) {
    use trit_parts::occ::Mlp;
    use trit_parts::tex::tile;

    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(30000);
    let sigma: f32 = arg(argv, "--sigma")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.1);
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let eval_seed: u64 = arg(argv, "--eval-seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let _ = eval_seed; // --data 模式下评测用留出图片，不再需要 seed
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.03);
    let patch: usize = arg(argv, "--patch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let cond = argv.iter().any(|a| a == "--cond");
    let r = (patch / 2) as i32;
    let kinds = ["wood", "marble", "rust", "grid"];
    let data_dir = arg(argv, "--data").map(PathBuf::from);
    let nin = patch * patch + if cond { kinds.len() } else { 0 };
    let hid: usize = arg(argv, "--hid")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);

    let gray = |kind: &str, sd: u64| -> Vec<f32> {
        tile(kind, n, sd)
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    };
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut nxt = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let gauss = |nxt: &mut dyn FnMut() -> f32| {
        let u1 = nxt().max(1e-6);
        let u2 = nxt();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    };
    let feats = |g: &[f32],
                 x: usize,
                 y: usize,
                 cond_idx: Option<usize>,
                 nxt: &mut dyn FnMut() -> f32|
     -> (Vec<f32>, f32) {
        let mut f = Vec::with_capacity(nin);
        for dy in -r..=r {
            for dx in -r..=r {
                let xx = ((x as i32 + dx).rem_euclid(n as i32)) as usize;
                let yy = ((y as i32 + dy).rem_euclid(n as i32)) as usize;
                f.push((g[yy * n + xx] + sigma * gauss(nxt)).clamp(0.0, 1.0));
            }
        }
        if let Some(ci) = cond_idx {
            for k in 0..kinds.len() {
                f.push((k == ci) as u8 as f32);
            }
        }
        (f, g[y * n + x])
    };

    // 数据源：--data <dir> 用真实材质（PNG，灰度，缩放到 n），否则用程序化材质
    let (images, names): (Vec<Vec<f32>>, Vec<String>) = match &data_dir {
        Some(dir) => {
            let mut v = Vec::new();
            let mut nm = Vec::new();
            let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
                .unwrap_or_else(|e| fail(&format!("读不了 {}: {e}", dir.display())))
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "png"))
                .collect();
            files.sort();
            for f in files {
                let (w0, h0, pix) = trit_parts::data::read_rgb(&f).unwrap_or_else(|e| fail(&e));
                let rgb = fit_rgb(w0, h0, &pix, n);
                let g: Vec<f32> = rgb
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|c| 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2])
                    .collect();
                nm.push(
                    f.file_stem()
                        .map(|x| x.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                );
                v.push(g);
            }
            if v.len() < 3 {
                fail("--data 至少要 3 张 PNG");
            }
            (v, nm)
        }
        None => (
            kinds.iter().map(|k| gray(k, seed)).collect(),
            kinds.iter().map(|k| (*k).to_string()).collect(),
        ),
    };
    // **固定留出**：默认按文件名排序取最前 2 张；扩数据集时新文件可能排到前面，
    // 所以跨规模实验要用 `--holdout-names` 显式钉住评测集（文件名前缀逗号分隔）。
    let hold_names: Vec<String> = arg(argv, "--holdout-names")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let n_eval = 2usize.min(images.len() - 1);
    let (eval_idx, all_train_idx): (Vec<usize>, Vec<usize>) = if hold_names.is_empty() {
        ((0..n_eval).collect(), (n_eval..images.len()).collect())
    } else {
        let mut ev = Vec::new();
        let mut tr = Vec::new();
        for (i, nm) in names.iter().enumerate() {
            if hold_names.iter().any(|h| nm.starts_with(h.as_str())) {
                ev.push(i);
            } else {
                tr.push(i);
            }
        }
        if ev.is_empty() || tr.len() < 2 {
            fail("--holdout-names 没匹配到文件（或训练集不足）");
        }
        (ev, tr)
    };
    let train_cap: usize = arg(argv, "--train-imgs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let train_idx: Vec<usize> = if train_cap > 0 && train_cap < all_train_idx.len() {
        all_train_idx[..train_cap].to_vec()
    } else {
        all_train_idx
    };

    let mut train: Vec<(Vec<f32>, f32)> = Vec::new();
    for (ki, &ii) in train_idx.iter().enumerate() {
        let g = &images[ii];
        for y in 0..n {
            for x in 0..n {
                let (f, t) = feats(
                    g,
                    x,
                    y,
                    if cond { Some(ki % kinds.len()) } else { None },
                    &mut nxt,
                );
                train.push((f, t));
            }
        }
    }
    let mut mlp = Mlp::new(nin, hid, seed);
    let mut loss = 0.0f64;
    for step in 0..iters {
        let (x, y) = &train[(step * 7919) % train.len()];
        loss += mlp.train_step(x, *y, lr) as f64;
        if (step + 1) % 5000 == 0 {
            println!(
                "  step {} loss {:.4} 三值零占比 {:.2}",
                step + 1,
                loss / 5000.0,
                mlp.sparsity()
            );
            loss = 0.0;
        }
    }

    let psnr_of = |mlp: &Mlp, g: &[f32], ci: Option<usize>, nxt: &mut dyn FnMut() -> f32| -> f64 {
        let (mut se_out, mut cnt) = (0.0f64, 0usize);
        for y in 0..n {
            for x in 0..n {
                let (f, clean) = feats(g, x, y, ci, nxt);
                let (logit, _) = mlp.forward(&f);
                let pred = 1.0 / (1.0 + (-logit).exp());
                se_out += f64::from((pred - clean).powi(2));
                cnt += 1;
            }
        }
        10.0 * (1.0 / (se_out / cnt as f64)).log10()
    };
    let mut good = Vec::new();
    let mut wrong = Vec::new();
    let eval_list: Vec<(String, &Vec<f32>)> = eval_idx
        .iter()
        .enumerate()
        .map(|(i, &ii)| {
            if data_dir.is_some() {
                (names[ii].clone(), &images[ii])
            } else {
                (format!("holdout{i}"), &images[ii])
            }
        })
        .collect();
    for (ki, (name, g)) in eval_list.iter().enumerate() {
        let g = *g;
        let p_ok = psnr_of(&mlp, g, if cond { Some(ki) } else { None }, &mut nxt);
        let p_bad = if cond {
            psnr_of(&mlp, g, Some((ki + 1) % kinds.len()), &mut nxt)
        } else {
            f64::NAN
        };
        println!(
            "留出 {name}: 正确条件 {p_ok:.2} dB{}",
            if cond {
                format!("  错误条件 {p_bad:.2} dB")
            } else {
                String::new()
            }
        );
        good.push(p_ok);
        if cond {
            wrong.push(p_bad);
        }
    }
    println!(
        "N5 学习地基：patch {}×{}，条件 {}，{} 步；留出 PSNR 均值 {:.2} dB{}",
        patch,
        patch,
        if cond { "开" } else { "关" },
        iters,
        avg(&good),
        if cond {
            format!(
                "；错条件 {:.2} dB（差 {:.2}）",
                avg(&wrong),
                avg(&good) - avg(&wrong)
            )
        } else {
            String::new()
        }
    );
}

/// `tex-conv --hidden 8,8 --iters 20000 [--cond]`：三值卷积栈（3×3、same、ReLU、sigmoid）去噪，
/// 与 `tex-prior` 的 3×3 MLP 基线在同一任务/同一协议下对照（留出材质种子、正确 vs 错误条件）。
pub(crate) fn tex_conv_cmd(argv: &[String]) {
    use trit_parts::tconv::Net;
    use trit_parts::tex::tile;

    let iters: usize = arg(argv, "--iters")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20000);
    let sigma: f32 = arg(argv, "--sigma")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.1);
    let n: usize = arg(argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let seed: u64 = arg(argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let eval_seed: u64 = arg(argv, "--eval-seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let lr: f32 = arg(argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.05);
    let cond = argv.iter().any(|a| a == "--cond");
    let use_bn = argv.iter().any(|a| a == "--bn");
    let adam = argv.iter().any(|a| a == "--adam");
    let bsz: usize = arg(argv, "--batch")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let hidden: Vec<usize> = arg(argv, "--hidden")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![8, 8]);

    let kinds = ["wood", "marble", "rust", "grid"];
    let gray = |kind: &str, sd: u64| -> Vec<f32> {
        tile(kind, n, sd)
            .iter()
            .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0)
            .collect()
    };
    let mut rng: u64 = 0x9E37_79B9_7F4A_7C15 ^ seed;
    let mut nxt = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f32 / (1u64 << 53) as f32
    };
    let noisy = |clean: &[f32], nxt: &mut dyn FnMut() -> f32| -> Vec<f32> {
        clean
            .iter()
            .map(|v| {
                let u1 = nxt().max(1e-6);
                let u2 = nxt();
                let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                (v + sigma * z).clamp(0.0, 1.0)
            })
            .collect()
    };
    let cin = 1 + if cond { kinds.len() } else { 0 };
    let mut net = Net::new(cin, &hidden, seed);
    net.cond = if cond { kinds.len() } else { 0 };
    net.set_bn(use_bn);
    let mut loss = 0.0f64;
    for step in 0..iters {
        let mut batch = Vec::with_capacity(bsz);
        for b in 0..bsz {
            let ki = (step * 7919 + b * 104729) % kinds.len();
            let sd = seed + ki as u64 + (step / kinds.len()) as u64; // 同种类换种子
            let clean = gray(kinds[ki], sd);
            let inp = noisy(&clean, &mut nxt);
            batch.push((inp, clean, if cond { Some(ki) } else { None }));
        }
        loss += net.train_batch(&batch, n, n, lr, adam) as f64;
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
    }
    // 参考图由**图像种类**决定，条件只影响模型输入（正确/错误条件对比必须保持参考不变）
    let eval = |net: &Net,
                img_kind: usize,
                cond_idx: Option<usize>,
                sd: u64,
                nxt: &mut dyn FnMut() -> f32|
     -> f64 {
        let clean = gray(kinds[img_kind], sd);
        let inp = noisy(&clean, nxt);
        let pred = net.forward(&inp, cond_idx, n, n);
        let (mut se, mut cnt) = (0.0f64, 0usize);
        for i in 0..n * n {
            se += f64::from((pred[i] - clean[i]).powi(2));
            cnt += 1;
        }
        10.0 * (1.0 / (se / cnt as f64)).log10()
    };
    let mut good = Vec::new();
    let mut wrong = Vec::new();
    for (ki, k) in kinds.iter().enumerate() {
        let sd = eval_seed + ki as u64;
        let p_ok = eval(&net, ki, if cond { Some(ki) } else { None }, sd, &mut nxt);
        let p_bad = if cond {
            eval(&net, ki, Some((ki + 1) % kinds.len()), sd, &mut nxt)
        } else {
            f64::NAN
        };
        println!("留出 {k}（seed {eval_seed}）: 正确条件 {p_ok:.2} dB  错误条件 {p_bad:.2} dB");
        good.push(p_ok);
        if cond {
            wrong.push(p_bad);
        }
    }
    println!(
        "三值卷积栈 hidden {:?}，BN {}，Adam {}，batch {}，条件 {}，{} 步；留出 PSNR 均值 {:.2} dB{}",
        hidden,
        if use_bn { "开" } else { "关" },
        if adam { "开" } else { "关" },
        bsz,
        if cond { "开" } else { "关" },
        iters,
        avg(&good),
        if cond {
            format!(
                "；错条件 {:.2} dB（差 {:.2}）",
                avg(&wrong),
                avg(&good) - avg(&wrong)
            )
        } else {
            String::new()
        }
    );
}

/// `tex-gen`：N5 生成第一块——DDIM 采样器 + 3×3 补丁去噪器，条件 = 材质 one-hot。
/// 训练在程序化材质上（随机像素 + 随机时间步），采样后用三项统计量对照真实材质：
/// 像素均值/方差、平铺接缝比、离最近真实补丁的距离（太小 = 抄训练集）。
/// tex-gen 里的三种去噪器；ε 模式要用**ε-空间的稳定采样器**，不能走 x0 形式的 DDIM。
pub(crate) enum Den {
    M(PatchMlp),
    C(Net),
    E(trit_parts::tconv::EpsDen),
    V(trit_parts::tconv::VDen),
}

impl Denoiser for Den {
    fn predict_x0(
        &self,
        xt: &[f32],
        t_frac: f32,
        cond: Option<usize>,
        w: usize,
        h: usize,
    ) -> Vec<f32> {
        match self {
            Den::M(d) => d.predict_x0(xt, t_frac, cond, w, h),
            Den::C(d) => d.predict_x0(xt, t_frac, cond, w, h),
            Den::E(d) => d.predict_x0(xt, t_frac, cond, w, h),
            Den::V(d) => d.predict_x0(xt, t_frac, cond, w, h),
        }
    }
    fn input_dim(&self) -> usize {
        match self {
            Den::M(d) => d.input_dim(),
            Den::C(d) => d.input_dim(),
            Den::E(d) => d.input_dim(),
            Den::V(d) => d.input_dim(),
        }
    }
}

impl Den {
    /// `eps_space=true` 走 ε-空间更新（高 t 系数放大误差，实测会饱和，仅作对照）。
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn sample(
        &self,
        sched: &Schedule,
        w: usize,
        h: usize,
        seed: u64,
        cond: Option<usize>,
        eps_space: bool,
        clamp: bool,
    ) -> Vec<f32> {
        match self {
            Den::E(e) if eps_space => {
                trit_parts::tconv::ddim_sample_eps(&e.net, &e.abar, w, h, seed, e.eps_scale)
            }
            _ => trit_parts::diffuse::ddim_sample_opts(self, sched, cond, w, h, seed, clamp),
        }
    }
}
