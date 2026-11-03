//! `parts-depth-gpu cnn-train`：三值卷积深度头的 **GPU** 训练。
//!
//! 与 `parts-depth cnn-train` 共用同一份样本预处理（[`trit_parts::depthdata`]）与
//! 同一套参数口径（`out = scale·(b + conv(q(w)))`），产物仍是 DCN3，
//! 可直接用 `parts-depth cnn-train --eval-only <dcn>` 评留出集。
//!
//! 例：
//! ```text
//! parts-depth-gpu cnn-train --data E:/trit-data/s1/mat60/renders/scene0000 \
//!   --size 64 --hidden 32 --dil 2 --epochs 2 --out E:/tmp/x.dcn --parity 3
//! ```

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use trit_parts::depthdata::{is_held, load_sample};
use trit_parts::depthgpu::{DepthGpu, TrainRef};
use trit_parts::depthnet::DepthNet;
use trit_parts::gpu::Gpu;

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn flag(argv: &[String], k: &str) -> bool {
    argv.iter().any(|a| a == k)
}

fn usage() -> ! {
    eprintln!(
        "用法: parts-depth-gpu cnn-train --data <dir,...> [--holdout a,b] [--size 64] \
         [--hidden 32] [--dil 2] [--epochs 60] [--lr 0.01] [--stride 2] [--pyr 0] [--full] \
         [--f32] [--seed 7] [--limit N] [--accum K] [--out model.dcn] [--parity N] \
         [--gpu-index N] [--list-gpus]\n\
         --accum K>1 改成 mini-batch SGD（每 K 个样本更新一次，梯度块内平均）；默认 1 = 与 CPU 逐样本同配方"
    );
    std::process::exit(2);
}

struct Sample {
    scene: PathBuf,
    x: Vec<Vec<f32>>,
    y: Vec<f32>,
    mask: Vec<bool>,
}

/// 与 CPU 完全相同的逐图归一化：用有效像素的中位/IQR 把 log 深度拉到 ~N(0,1)。
fn norm(y: &mut [f32], mask: &[bool]) -> (f32, f32) {
    let mut v: Vec<f32> = y
        .iter()
        .zip(mask)
        .filter(|(_, m)| **m)
        .map(|(a, _)| *a)
        .collect();
    v.sort_by(f32::total_cmp);
    let q = |f: f32| v[((v.len() as f32 - 1.0) * f).round() as usize];
    let (med, iqr) = (q(0.5), (q(0.75) - q(0.25)).max(1e-3));
    for (x, m) in y.iter_mut().zip(mask) {
        *x = if *m {
            ((*x - med) / iqr).clamp(-4.0, 4.0)
        } else {
            0.0
        };
    }
    (med, iqr)
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    // 探测卡数不需要子命令，也要在 `--data` 解析之前处理。
    if flag(&argv, "--list-gpus") {
        for (i, name) in Gpu::adapters().iter().enumerate() {
            println!("{i}: {name}");
        }
        return;
    }
    if argv.first().map(String::as_str) != Some("cnn-train") {
        usage();
    }
    let data: Vec<PathBuf> = arg(&argv, "--data")
        .unwrap_or_else(|| usage())
        .split(',')
        .map(PathBuf::from)
        .collect();
    let size: usize = arg(&argv, "--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(64);
    let epochs: usize = arg(&argv, "--epochs")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let lr: f32 = arg(&argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let hidden: usize = arg(&argv, "--hidden")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);
    let stride: usize = arg(&argv, "--stride")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let pyr: usize = arg(&argv, "--pyr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let dil: usize = arg(&argv, "--dil")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let seed: u64 = arg(&argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let limit: usize = arg(&argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let accum: usize = arg(&argv, "--accum")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1)
        .max(1);
    let parity: usize = arg(&argv, "--parity")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let out = arg(&argv, "--out").map(PathBuf::from);
    let holdout: Vec<String> = arg(&argv, "--holdout")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();
    let full = flag(&argv, "--full");
    let ternary = !flag(&argv, "--f32");

    let mut net = DepthNet::with_inputs(3 + 3 * pyr, hidden, seed, ternary);
    net.set_dilation(dil);

    let gpu = match arg(&argv, "--gpu-index").and_then(|v| v.parse::<usize>().ok()) {
        Some(i) => {
            Gpu::new_on(i).unwrap_or_else(|| panic!("GPU {i} 不可用（先用 --list-gpus 看）"))
        }
        None => Gpu::new().expect("没有可用 GPU"),
    };
    println!("使用适配器：{}", gpu.adapter);
    println!(
        "卷积深度头（GPU）：{} 权重，dil {dil}（补零核 {}），输出层补齐到 4 通道",
        if ternary { "三值" } else { "f32" },
        2 * dil + 1
    );
    if accum > 1 {
        println!("--accum {accum}：mini-batch SGD（每 {accum} 个样本更新一次，梯度块内平均）");
    }
    let mut gnet = DepthGpu::from_depthnet(&net, size as u32).with_slots(accum);
    gnet.upload(&gpu);

    // 样本（与 CPU 同一份预处理）
    let mut samples: Vec<Sample> = Vec::new();
    for d in &data {
        for fr in 0..=24 {
            if fr % stride.max(1) != 0 {
                continue;
            }
            let Some((x, y, mask, _med, _iqr, _w0)) = load_sample(d, fr, size, full, pyr) else {
                if fr == 0 {
                    break; // 这个目录没有帧 0（不是场景目录）
                }
                continue;
            };
            samples.push(Sample {
                scene: d.clone(),
                x,
                y,
                mask,
            });
            if limit > 0 && samples.len() >= limit {
                break;
            }
        }
        if limit > 0 && samples.len() >= limit {
            break;
        }
    }
    if samples.is_empty() {
        panic!("没有可用样本");
    }
    let (mut train, _held): (Vec<_>, Vec<_>) = samples
        .into_iter()
        .partition(|s| !is_held(&s.scene, &holdout));
    let mut med_sum = 0.0f32;
    let mut iqr_sum = 0.0f32;
    for s in train.iter_mut() {
        let (m, q) = norm(&mut s.y, &s.mask);
        med_sum += m;
        iqr_sum += q;
    }
    gnet.med = med_sum / train.len() as f32;
    gnet.iqr = iqr_sum / train.len() as f32;
    println!(
        "样本 {}（训练 {}；{} 场景，{}²，hidden {}），逐图统计均值 med {:.4} iqr {:.4}",
        train.len() + _held.len(),
        train.len(),
        data.len(),
        size,
        hidden,
        gnet.med,
        gnet.iqr
    );

    if parity > 0 {
        let s = &train[0];
        let cpu = net.forward(&s.x, size, size);
        let gpu_out = gnet.predict(&gpu, &s.x);
        let md = cpu
            .iter()
            .zip(&gpu_out)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        println!("前向对拍：max|Δ| = {md:.3e}");
        // 同步数训练对拍：同一初始化、同样本、同 lr
        let mut first: Vec<(&'static str, f32)> = Vec::new();
        for step in 0..parity {
            let s = &train[step % train.len()];
            let lc = net.train_step(&s.x, size, size, &s.y, &s.mask, lr);
            let lg = gnet.train_step(&gpu, &s.x, &s.y, &s.mask, lr);
            if step == 0 || step + 1 == parity {
                println!(
                    "  对拍 step {}：CPU loss {lc:.6} / GPU loss {lg:.6}",
                    step + 1
                );
            }
            if step == 0 {
                first = param_diffs(&gnet, &net);
            }
        }
        let last = param_diffs(&gnet, &net);
        println!("参数对拍（1 步后，判据）：{}", fmt_diffs(&first));
        println!("参数对拍（{parity} 步后，信息性）：{}", fmt_diffs(&last));
        // 判据只看 **1 步**：Adam 每步步长 ≈ lr，多步后近零梯度翻符号会让参数差到 1e-3 量级
        //（三值层还有 |w|≈0.5 的整格翻转），那是指令级噪声，不是实现错。
        if first.iter().any(|(_, d)| *d > 1e-4) {
            eprintln!("对拍失败：1 步后参数偏差 > 1e-4");
            std::process::exit(1);
        }
        // 对拍后回到同一起点继续正式训练
        gnet = DepthGpu::from_depthnet(
            &DepthNet::with_inputs(3 + 3 * pyr, hidden, seed, ternary),
            size as u32,
        )
        .with_slots(accum);
        gnet.med = med_sum / train.len() as f32;
        gnet.iqr = iqr_sum / train.len() as f32;
        gnet.upload(&gpu);
    }

    let mut last_mse = 0.0f32;
    for ep in 0..epochs {
        let t0 = Instant::now();
        let mut loss = 0.0f32;
        if accum <= 1 {
            for s in &train {
                loss += gnet.train_step(&gpu, &s.x, &s.y, &s.mask, lr);
            }
        } else {
            for chunk in train.chunks(accum) {
                let b: Vec<TrainRef> = chunk
                    .iter()
                    .map(|s| (s.x.as_slice(), s.y.as_slice(), s.mask.as_slice()))
                    .collect();
                loss += gnet.train_block(&gpu, &b, lr);
            }
        }
        let mse = loss / train.len() as f32;
        last_mse = mse;
        println!(
            "epoch {}/{}：MSE {:.4}（{:.0} ms，{:.1} 样本/秒）",
            ep + 1,
            epochs,
            mse,
            t0.elapsed().as_secs_f64() * 1000.0,
            train.len() as f64 / t0.elapsed().as_secs_f64()
        );
    }
    if let Some(p) = out {
        gnet.to_depthnet()
            .save(&p)
            .unwrap_or_else(|e| panic!("{e}"));
        println!("已存 {}", p.display());
        write_manifest(
            &p,
            &argv,
            &data,
            &holdout,
            last_mse,
            train.len(),
            _held.len(),
        );
    }
    if flag(&argv, "--profile") {
        let p = gnet.prof;
        let tot: f64 = p.iter().sum();
        let names = ["打包+前向+读回", "造 dy", "反向 dispatch", "读回+更新+上传"];
        for (n, v) in names.iter().zip(&p) {
            println!("  {n:<18} {v:6.2} s  {:5.1}%", v / tot.max(1e-9) * 100.0);
        }
        println!(
            "  合计 {tot:.2} s，{:.2} ms/样本",
            tot * 1000.0 / (epochs * train.len()).max(1) as f64
        );
    }
}

fn param_diffs(g: &DepthGpu, n: &DepthNet) -> Vec<(&'static str, f32)> {
    let back = g.to_depthnet();
    vec![
        ("c1.w", maxdiff(&back.c1.w, &n.c1.w)),
        ("c2.w", maxdiff(&back.c2.w, &n.c2.w)),
        ("c3.w", maxdiff(&back.c3.w, &n.c3.w)),
        ("c1.b", maxdiff(&back.c1.b, &n.c1.b)),
        ("c2.b", maxdiff(&back.c2.b, &n.c2.b)),
        ("c1.s", maxdiff(&back.c1.scale, &n.c1.scale)),
        ("c2.s", maxdiff(&back.c2.scale, &n.c2.scale)),
        ("c3.s", maxdiff(&back.c3.scale, &n.c3.scale)),
        ("out_b", (back.out_b - n.out_b).abs()),
    ]
}

fn fmt_diffs(d: &[(&'static str, f32)]) -> String {
    d.iter()
        .map(|(n, d)| format!("{n} {d:.2e}"))
        .collect::<Vec<_>>()
        .join("  ")
}

fn maxdiff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}

/// JSON 字符串转义（不引 serde）。
fn jstr(s: &str) -> String {
    let bs = '\\';
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => {
                o.push(bs);
                o.push('"');
            }
            c if c == bs => {
                o.push(bs);
                o.push(bs);
            }
            '\n' => {
                o.push(bs);
                o.push('n');
            }
            '\r' => {
                o.push(bs);
                o.push('r');
            }
            '\t' => {
                o.push(bs);
                o.push('t');
            }
            c if (c as u32) < 0x20 => o.push_str(&format!("{}u{:04x}", bs, c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn git_out(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// 写 `<model>.manifest.json`：结果族台账的机器可读那一半。
fn write_manifest(
    out: &Path,
    argv: &[String],
    data: &[PathBuf],
    holdout: &[String],
    last_mse: f32,
    train: usize,
    held: usize,
) {
    let model_sha = trit_parts::fingerprint::file_sha256(out).unwrap_or_default();
    let stride = arg(argv, "--stride")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(2);
    let (dirs, files, bytes, data_hash) = trit_parts::depthdata::data_fingerprint(data, stride);
    let commit = git_out(&["rev-parse", "--short", "HEAD"])
        .trim()
        .to_string();
    let dirty = !git_out(&["status", "--porcelain"]).trim().is_empty();
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let num = |k: &str, d: f64| {
        arg(argv, k)
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(d)
    };
    let epochs = num("--epochs", 3.0) as u32;
    let config = format!(
        "{{\"size\": {}, \"hidden\": {}, \"dil\": {}, \"epochs\": {epochs}, \"lr\": {}, \"accum\": {}, \"stride\": {}, \"pyr\": {}, \"full\": {}, \"f32\": {}, \"seed\": {}}}",
        num("--size", 64.0) as u32,
        num("--hidden", 8.0) as u32,
        num("--dil", 1.0) as u32,
        num("--lr", 0.01),
        num("--accum", 1.0) as u32,
        num("--stride", 2.0) as u32,
        num("--pyr", 0.0) as u32,
        flag(argv, "--full"),
        flag(argv, "--f32"),
        num("--seed", 7.0) as u64
    );
    let argv_json: Vec<String> = argv.iter().map(|a| jstr(a)).collect();
    let hold_json: Vec<String> = holdout.iter().map(|a| jstr(a)).collect();
    let model_bytes = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"depth-cnn-train\",\n  \"created_unix\": {created},\n  \"commit\": {},\n  \"dirty\": {dirty},\n  \"argv\": [{}],\n  \"config\": {config},\n  \"data\": {{\"dirs\": {dirs}, \"files\": {files}, \"bytes\": {bytes}, \"algo\": \"sha256-content-v1\", \"hash\": \"sha256:{data_hash}\", \"train\": {train}, \"held\": {held}}},\n  \"holdout\": [{}],\n  \"model\": {{\"path\": {}, \"sha256\": \"{model_sha}\", \"bytes\": {model_bytes}}},\n  \"result\": {{\"final_mse\": {last_mse}, \"epochs\": {epochs}}}\n}}\n",
        jstr(&commit),
        argv_json.join(", "),
        hold_json.join(", "),
        jstr(&out.to_string_lossy())
    );
    let mut mpath = out.as_os_str().to_owned();
    mpath.push(".manifest.json");
    let mpath = PathBuf::from(mpath);
    std::fs::write(&mpath, body).unwrap_or_else(|e| panic!("{}: {e}", mpath.display()));
    println!("清单 {}", mpath.display());
}
