//! `parts-ood`：域外拒识（阶段 1）的训练与评估。
//!
//! - `train --model <tpm> --manifest <json> --pos <dir,...> --neg <dir,...> --out <tpod>`
//! - `eval --model <tpm> --manifest <json> --bank <tpod> --pos <dir,...> --neg <dir,...>`

use std::path::{Path, PathBuf};

use trit_parts::data::read_rgb;
use trit_parts::gpu::Gpu;
use trit_parts::net::Net;
use trit_parts::ood::{NFEAT, OodBank, auc, features, tpr_at_fpr};
use trit_parts::parts::PartList;
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

fn images(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|x| x == "png" || x == "jpg" || x == "jpeg")
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// 双线性缩放到 `res²`（负样本尺寸任意）。
fn to_res(w: usize, h: usize, pix: &[f32], res: usize) -> Vec<f32> {
    if w == res && h == res {
        return pix.to_vec();
    }
    let mut out = vec![0f32; res * res * 3];
    for y in 0..res {
        let fy = (y as f32 + 0.5) * h as f32 / res as f32 - 0.5;
        let y0 = fy.floor().max(0.0) as usize;
        let y1 = (y0 + 1).min(h - 1);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        for x in 0..res {
            let fx = (x as f32 + 0.5) * w as f32 / res as f32 - 0.5;
            let x0 = fx.floor().max(0.0) as usize;
            let x1 = (x0 + 1).min(w - 1);
            let tx = (fx - x0 as f32).clamp(0.0, 1.0);
            for c in 0..3 {
                let a = pix[(y0 * w + x0) * 3 + c] * (1.0 - tx) + pix[(y0 * w + x1) * 3 + c] * tx;
                let b = pix[(y1 * w + x0) * 3 + c] * (1.0 - tx) + pix[(y1 * w + x1) * 3 + c] * tx;
                out[(y * res + x) * 3 + c] = a * (1.0 - ty) + b * ty;
            }
        }
    }
    out
}

fn flip(pix: &[f32], res: usize, vertical: bool) -> Vec<f32> {
    let mut out = vec![0f32; pix.len()];
    for y in 0..res {
        for x in 0..res {
            let (sy, sx) = if vertical {
                (res - 1 - y, x)
            } else {
                (y, res - 1 - x)
            };
            let d = (y * res + x) * 3;
            let s = (sy * res + sx) * 3;
            out[d..d + 3].copy_from_slice(&pix[s..s + 3]);
        }
    }
    out
}

fn brighten(pix: &[f32], k: f32) -> Vec<f32> {
    pix.iter().map(|v| (v * k).clamp(0.0, 1.0)).collect()
}

fn infer_feats(net: &Net, gpu: &Gpu, rgb: &[f32], res: usize, k: usize) -> Vec<f32> {
    let xb = gpu.buf(rgb, "x");
    let tr = net.forward(gpu, xb, res as u32);
    let hw = res * res;
    let ch = net.head_cout() as usize;
    let logits = gpu.read(&tr.out, hw * ch);
    let z = gpu.read(&tr.z, hw * net.layers[trit_parts::net::D1B].cout as usize);
    let mut ids = vec![0u32; hw];
    for p in 0..hw {
        if logits[p * ch + k * 3] <= 0.0 {
            continue;
        }
        let mut id = 0u32;
        for i in 0..k {
            let b = p * ch + i * 3;
            let (a, b2, c) = (logits[b], logits[b + 1], logits[b + 2]);
            let d = if a >= b2 && a >= c {
                0
            } else if b2 >= c {
                1
            } else {
                2
            };
            id = id * 3 + d;
        }
        ids[p] = id + 1;
    }
    features(&ids, &logits, &z, hw, k)
}

fn load_model(path: &Path) -> (Gpu, Net) {
    let gpu = Gpu::new().expect("GPU 初始化失败");
    let tpm = TpmModel::load(path).unwrap_or_else(|e| panic!("{e}"));
    let mut net = Net::from_tpm(&tpm);
    net.upload_all(&gpu);
    (gpu, net)
}

fn collect(
    net: &Net,
    gpu: &Gpu,
    k: usize,
    dirs: &[PathBuf],
    neg: bool,
    per_dir: usize,
    aug: bool,
) -> Vec<Vec<f32>> {
    let mut out = Vec::new();
    for d in dirs {
        let mut files = if neg { images(d) } else { frames(d) };
        if files.is_empty() {
            files = frames(d);
        }
        for p in files.into_iter().take(per_dir) {
            let (w, h, pix) = match read_rgb(&p) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("跳过 {}: {e}", p.display());
                    continue;
                }
            };
            let base = to_res(w, h, &pix, 384);
            out.push(infer_feats(net, gpu, &base, 384, k));
            if aug {
                out.push(infer_feats(net, gpu, &flip(&base, 384, false), 384, k));
                out.push(infer_feats(net, gpu, &flip(&base, 384, true), 384, k));
                out.push(infer_feats(net, gpu, &brighten(&base, 1.2), 384, k));
                out.push(infer_feats(net, gpu, &brighten(&base, 0.8), 384, k));
            }
        }
    }
    out
}

fn split_dirs(v: Option<String>) -> Vec<PathBuf> {
    v.unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
        .collect()
}

fn train(argv: &[String]) {
    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let pos = split_dirs(arg(argv, "--pos"));
    let neg = split_dirs(arg(argv, "--neg"));
    let per_dir: usize = arg(argv, "--per-dir")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let l2: f32 = arg(argv, "--l2")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1e-2);
    let epochs: usize = arg(argv, "--epochs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2000);
    let out = PathBuf::from(arg(argv, "--out").unwrap_or_else(|| usage()));

    let parts = PartList::load(&manifest).unwrap_or_else(|e| panic!("{e}"));
    let (gpu, net) = load_model(&model);
    let k = parts.k;

    let xs_pos = collect(&net, &gpu, k, &pos, false, per_dir, true);
    let xs_neg = collect(&net, &gpu, k, &neg, true, usize::MAX, true);
    println!("训练样本：域内 {} / 域外 {}", xs_pos.len(), xs_neg.len());

    let mut xs: Vec<Vec<f32>> = Vec::new();
    let mut ys: Vec<f32> = Vec::new();
    for x in &xs_pos {
        xs.push(x.clone());
        ys.push(1.0);
    }
    for x in &xs_neg {
        xs.push(x.clone());
        ys.push(0.0);
    }
    let mut bank = OodBank::zeros();
    bank.fit(&xs, &ys, l2, epochs);
    bank.save(&out).unwrap_or_else(|e| panic!("{e}"));

    let sc: Vec<f32> = xs.iter().map(|x| bank.score(x)).collect();
    let lab: Vec<bool> = ys.iter().map(|&v| v > 0.5).collect();
    println!(
        "训练集 AUC {:.4}；特征维 {}；已存 {}",
        auc(&sc, &lab),
        NFEAT,
        out.display()
    );
}

fn eval(argv: &[String]) {
    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let bank_p = PathBuf::from(arg(argv, "--bank").unwrap_or_else(|| usage()));
    let pos = split_dirs(arg(argv, "--pos"));
    let neg = split_dirs(arg(argv, "--neg"));
    let per_dir: usize = arg(argv, "--per-dir")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);

    let parts = PartList::load(&manifest).unwrap_or_else(|e| panic!("{e}"));
    let (gpu, net) = load_model(&model);
    let bank = OodBank::load(&bank_p).unwrap_or_else(|e| panic!("{e}"));
    let k = parts.k;

    let xs_pos = collect(&net, &gpu, k, &pos, false, per_dir, false);
    let xs_neg = collect(&net, &gpu, k, &neg, true, usize::MAX, false);

    let mut sc: Vec<f32> = Vec::new();
    let mut lab: Vec<bool> = Vec::new();
    for x in &xs_pos {
        sc.push(bank.score(x));
        lab.push(true);
    }
    for x in &xs_neg {
        sc.push(bank.score(x));
        lab.push(false);
    }
    let (tpr, th) = tpr_at_fpr(&sc, &lab, 0.01);
    let fp: usize = xs_neg
        .iter()
        .map(|x| bank.score(x))
        .filter(|&s| s >= th)
        .count();
    println!(
        "测试：域内 {} / 域外 {}；AUC {:.4}；TPR@FPR≤1% **{:.3}**（阈值 {:.3}，域外误收 {}/{}）",
        xs_pos.len(),
        xs_neg.len(),
        auc(&sc, &lab),
        tpr,
        th,
        fp,
        xs_neg.len()
    );
    let mut pos_s: Vec<f32> = xs_pos.iter().map(|x| bank.score(x)).collect();
    let mut neg_s: Vec<f32> = xs_neg.iter().map(|x| bank.score(x)).collect();
    pos_s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    neg_s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let q = |v: &[f32], f: f32| {
        if v.is_empty() {
            f32::NAN
        } else {
            v[((v.len() - 1) as f32 * f) as usize]
        }
    };
    println!(
        "分数分位：域内 min {:.3} p10 {:.3} 中位 {:.3}；域外 中位 {:.3} p90 {:.3} max {:.3}",
        q(&pos_s, 0.0),
        q(&pos_s, 0.1),
        q(&pos_s, 0.5),
        q(&neg_s, 0.5),
        q(&neg_s, 0.9),
        q(&neg_s, 1.0)
    );
}

/// `check --model --manifest --bank --image <png> [--threshold 0.5]`：单图闸门。
fn check(argv: &[String]) {
    let model = PathBuf::from(arg(argv, "--model").unwrap_or_else(|| usage()));
    let manifest = PathBuf::from(arg(argv, "--manifest").unwrap_or_else(|| usage()));
    let bank_p = PathBuf::from(arg(argv, "--bank").unwrap_or_else(|| usage()));
    let image = PathBuf::from(arg(argv, "--image").unwrap_or_else(|| usage()));
    let th: f32 = arg(argv, "--threshold")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.5);

    let parts = PartList::load(&manifest).unwrap_or_else(|e| panic!("{e}"));
    let (gpu, net) = load_model(&model);
    let bank = OodBank::load(&bank_p).unwrap_or_else(|e| panic!("{e}"));
    let (w, h, pix) = read_rgb(&image).unwrap_or_else(|e| panic!("{e}"));
    let base = to_res(w, h, &pix, 384);
    let f = infer_feats(&net, &gpu, &base, 384, parts.k);
    let score = bank.score(&f);
    let ok = score >= th;
    println!(
        "{} 域内概率 {:.4}（阈值 {:.3}）⇒ {}",
        image.display(),
        score,
        th,
        if ok { "接受" } else { "拒绝（域外）" }
    );
    if !ok {
        std::process::exit(4);
    }
}

fn usage() -> ! {
    eprintln!(
        "用法:
  parts-ood train --model <tpm> --manifest <json> --pos <dir,...> --neg <dir,...> --out <tpod>
  parts-ood eval  --model <tpm> --manifest <json> --bank <tpod> --pos <dir,...> --neg <dir,...>
  parts-ood check --model <tpm> --manifest <json> --bank <tpod> --image <png> [--threshold 0.5]"
    );
    std::process::exit(2);
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match argv.first().map(String::as_str) {
        Some("train") => train(&argv),
        Some("eval") => eval(&argv),
        Some("check") => check(&argv),
        _ => usage(),
    }
}
