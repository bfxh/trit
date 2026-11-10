//! `parts-depth-unet`：多尺度（降采样编码器）深度头的 **CPU** 训练与留出评估。
//!
//! 结构与 CPU 参考实现一致（[`trit_parts::depthunet::DepthUNet`]）：
//! `64² → 32² → 16²` 降采样编码 + 上采样解码 + 32² 跳连。
//! 目的：先回答「降采样编码器能不能破 8%」，有增益再上 GPU。
//!
//! 例：
//! ```text
//! parts-depth-unet --data E:/trit-data/s1/mat60/renders/scene0000 --size 64 \
//!   --hidden 16 --epochs 60 --out E:/tmp/depth600/unet.u1
//! ```

use std::path::PathBuf;
use std::time::Instant;

use trit_parts::depthdata::{EvalSample, eval_with, is_held, load_sample};
use trit_parts::depthunet::DepthUNet;

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
        "用法: parts-depth-unet --data <dir,...> [--holdout a,b] [--size 64] [--hidden 16] \
         [--epochs 60] [--lr 0.01] [--stride 2] [--pyr 0] [--ppyr 0] [--full] [--limit N] \
         [--seed 7] [--out model.u1]"
    );
    std::process::exit(2);
}

struct Sample {
    scene: PathBuf,
    x: Vec<Vec<f32>>,
    y: Vec<f32>,
    mask: Vec<bool>,
}

/// 与 `cnn-train` 同口径的逐图归一化（用有效像素的中位/IQR）。
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
        .unwrap_or(60);
    let lr: f32 = arg(&argv, "--lr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.01);
    let hidden: usize = arg(&argv, "--hidden")
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let stride: usize = arg(&argv, "--stride")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2);
    let pyr: usize = arg(&argv, "--pyr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let ppyr: usize = arg(&argv, "--ppyr")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let seed: u64 = arg(&argv, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(7);
    let limit: usize = arg(&argv, "--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let full = flag(&argv, "--full");
    let out = arg(&argv, "--out").map(PathBuf::from);
    let holdout: Vec<String> = arg(&argv, "--holdout")
        .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default();

    let mut net = DepthUNet::new(3 + 3 * pyr + 3 * ppyr, hidden, seed, true);
    println!(
        "降采样编码器（CPU）：h {hidden}（降采样支路 2h），输入 {} 通道",
        3 + 3 * pyr + 3 * ppyr
    );

    let mut samples: Vec<Sample> = Vec::new();
    for d in &data {
        for fr in 0..=24 {
            if fr % stride.max(1) != 0 {
                continue;
            }
            let Some((x, y, mask, _m, _q, _w)) = load_sample(d, fr, size, full, pyr, ppyr) else {
                if fr == 0 {
                    break;
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
    let (mut train, held): (Vec<_>, Vec<_>) = samples
        .into_iter()
        .partition(|s| !is_held(&s.scene, &holdout));
    let mut med_sum = 0.0f32;
    let mut iqr_sum = 0.0f32;
    for s in train.iter_mut() {
        let (m, q) = norm(&mut s.y, &s.mask);
        med_sum += m;
        iqr_sum += q;
    }
    net.med = med_sum / train.len() as f32;
    net.iqr = iqr_sum / train.len() as f32;
    println!(
        "样本 {}（训练 {} / 留出 {}；{} 场景，{}²），med {:.4} iqr {:.4}",
        train.len() + held.len(),
        train.len(),
        held.len(),
        data.len(),
        size,
        net.med,
        net.iqr
    );

    for ep in 0..epochs {
        let t0 = Instant::now();
        let mut loss = 0.0f32;
        for s in &train {
            loss += net.train_step(&s.x, size, size, &s.y, &s.mask, lr);
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
    if let Some(p) = &out {
        net.save(p).unwrap_or_else(|e| panic!("{e}"));
        println!("已存 {}", p.display());
    }
    if !held.is_empty() {
        let es: Vec<EvalSample> = held
            .iter()
            .map(|s| EvalSample {
                x: s.x.clone(),
                y: s.y.clone(),
                mask: s.mask.clone(),
            })
            .collect();
        let r = eval_with(|x| net.forward(x, size, size), net.med, net.iqr, &es, size);
        println!(
            "留出 {} 帧 / {} 像素：形状对齐 log MAE {:.4}、深度相对误差 {:.2}%（常数基线 {:.2}%），δ<1.25 {:.3}；默认尺度相对误差 {:.2}%",
            r.frames, r.pixels, r.mae_log, r.rel_pct, r.base_pct, r.delta, r.deploy_pct
        );
    }
}
