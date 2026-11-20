//! 复现《WSP Doesn't Need Transformers》的二阶联想记忆读取器。

use std::path::Path;

mod apply;
mod npz;
mod gpu;
mod gpu_prep;
mod model;
mod model_fixed;
mod quant;
mod quantize;
mod rows;
mod topk;
mod weights;

use model::{O_LEN, Q_LEN, argmax, forward};
use crate::{npz::Npz, weights::{load_weights, read_i64}};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("用法：cargo run --release -- <weights.npz> <arc_test.npz> [--gpu]");
        std::process::exit(2);
    }
    let mut w = load_weights(Path::new(&args[1]));
    apply::run(&mut w);

    let mut npz =
        Npz::open(&args[2]).unwrap_or_else(|e| panic!("打开 {} 失败：{e}", args[2]));
    let q = read_i64(&mut npz, "q");
    let o = read_i64(&mut npz, "o");
    let y = read_i64(&mut npz, "y");
    let n = y.len();
    assert_eq!(q.len(), n * Q_LEN, "q 形状不对");
    assert_eq!(o.len(), n * 4 * O_LEN, "o 形状不对");
    println!(
        "概念 {}｜词表 {}｜二阶表 {}×{}｜测试题 {}",
        w.c,
        w.concept.len(),
        w.h,
        w.r,
        n
    );

    if args.iter().any(|a| a == "--gpu") {
        gpu::report(&w, &q, &o, &y, n);
        return;
    }

    let mut right = 0usize;
    let mut answered = 0usize;
    let started = std::time::Instant::now();
    for i in 0..n {
        let logits = forward(
            &w,
            &q[i * Q_LEN..(i + 1) * Q_LEN],
            &o[i * 4 * O_LEN..(i + 1) * 4 * O_LEN],
        );
        let is_answered = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            > logits.iter().cloned().fold(f32::INFINITY, f32::min);
        if is_answered {
            answered += 1;
            if argmax(&logits) == y[i] as usize {
                right += 1;
            }
        }
    }
    let nf = n as f64;
    let accuracy = right as f64 / nf + (n - answered) as f64 / nf / 4.0;
    println!(
        "accuracy {accuracy:.16}｜answered {:.16}｜n={n}｜{:.2} ms",
        answered as f64 / nf,
        started.elapsed().as_secs_f64() * 1000.0
    );
}
