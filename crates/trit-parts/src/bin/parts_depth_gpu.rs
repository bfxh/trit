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

use std::path::PathBuf;
use std::time::Instant;

use trit_parts::depthdata::{is_held, load_sample};
use trit_parts::depthgpu::DepthGpu;
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
         [--f32] [--seed 7] [--limit N] [--out model.dcn] [--parity N]"
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

    let gpu = Gpu::new().expect("没有可用 GPU");
    println!("使用适配器：{}", gpu.adapter);
    println!(
        "卷积深度头（GPU）：{} 权重，dil {dil}（补零核 {}），输出层补齐到 4 通道",
        if ternary { "三值" } else { "f32" },
        2 * dil + 1
    );
    let mut gnet = DepthGpu::from_depthnet(&net, size as u32);
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
        }
        let back = gnet.to_depthnet();
        let diffs = [
            ("c1.w", maxdiff(&back.c1.w, &net.c1.w)),
            ("c2.w", maxdiff(&back.c2.w, &net.c2.w)),
            ("c3.w", maxdiff(&back.c3.w, &net.c3.w)),
            ("c1.b", maxdiff(&back.c1.b, &net.c1.b)),
            ("c2.b", maxdiff(&back.c2.b, &net.c2.b)),
            ("c1.s", maxdiff(&back.c1.scale, &net.c1.scale)),
            ("c2.s", maxdiff(&back.c2.scale, &net.c2.scale)),
            ("c3.s", maxdiff(&back.c3.scale, &net.c3.scale)),
            ("out_b", (back.out_b - net.out_b).abs()),
        ];
        println!(
            "参数对拍（{parity} 步后）：{}",
            diffs
                .iter()
                .map(|(n, d)| format!("{n} {d:.2e}"))
                .collect::<Vec<_>>()
                .join("  ")
        );
        if diffs.iter().any(|(_, d)| *d > 1e-3) {
            eprintln!("对拍失败：参数偏差 > 1e-3");
            std::process::exit(1);
        }
        // 对拍后回到同一起点继续正式训练
        gnet = DepthGpu::from_depthnet(
            &DepthNet::with_inputs(3 + 3 * pyr, hidden, seed, ternary),
            size as u32,
        );
        gnet.med = med_sum / train.len() as f32;
        gnet.iqr = iqr_sum / train.len() as f32;
        gnet.upload(&gpu);
    }

    for ep in 0..epochs {
        let t0 = Instant::now();
        let mut loss = 0.0f32;
        for s in &train {
            loss += gnet.train_step(&gpu, &s.x, &s.y, &s.mask, lr);
        }
        println!(
            "epoch {}/{}：MSE {:.4}（{:.0} ms，{:.1} 样本/秒）",
            ep + 1,
            epochs,
            loss / train.len() as f32,
            t0.elapsed().as_secs_f64() * 1000.0,
            train.len() as f64 / t0.elapsed().as_secs_f64()
        );
    }
    if let Some(p) = out {
        gnet.to_depthnet()
            .save(&p)
            .unwrap_or_else(|e| panic!("{e}"));
        println!("已存 {}", p.display());
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

fn maxdiff(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y).abs())
        .fold(0.0f32, f32::max)
}
