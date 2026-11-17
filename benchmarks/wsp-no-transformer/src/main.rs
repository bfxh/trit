//! 复现《Weight-Space Programming Doesn't Need Transformers》的二阶联想记忆读取器
//! （`models/<writer>/mem_assoc2*/arch.py` 的 `forward`）。权重由 Python 的 `export_baseline.py`
//! 导出；本程序只做 forward + ARC test 评测，用来验证 Rust 与论文读数逐题一致。
//! 运行：`cargo run --release -- <weights.npz> <arc_test.npz>`。

use std::path::Path;

use npyz::npz::NpzArchive;

mod model;
mod quant;
mod quantize;
mod weights;

use model::{O_LEN, Q_LEN, argmax, forward};
use quant::Quant;
use quantize::quantize_matrix;
use weights::{load_weights, read_i64};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!("用法：cargo run --release -- <weights.npz> <arc_test.npz> [logits.bin]");
        std::process::exit(2);
    }
    let mut w = load_weights(Path::new(&args[1]));
    let quant = Quant::from_env_var("WSP_QUANT");
    let quant_row =
        std::env::var("WSP_QUANT_ROW").map_or(quant, |_| Quant::from_env_var("WSP_QUANT_ROW"));
    let target = std::env::var("WSP_QUANT_TARGET").unwrap_or_else(|_| "both".to_string());
    if target == "a" || target == "both" {
        quantize_matrix(&mut w.a, w.c, w.c, quant);
    }
    if target == "row" || target == "both" {
        quantize_matrix(&mut w.row_w, w.h, w.r, quant_row);
    }
    println!(
        "量化 A={quant:?} row={quant_row:?}｜目标 {target}｜A {}×{}｜二阶表 {}×{}",
        w.c, w.c, w.h, w.r
    );

    let mut npz =
        NpzArchive::open(&args[2]).unwrap_or_else(|e| panic!("打开 {} 失败：{e}", args[2]));
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

    let mut right = 0usize;
    let mut answered = 0usize;
    let mut all = Vec::with_capacity(n * 4);
    let started = std::time::Instant::now();
    for i in 0..n {
        let logits = forward(
            &w,
            &q[i * Q_LEN..(i + 1) * Q_LEN],
            &o[i * 4 * O_LEN..(i + 1) * 4 * O_LEN],
        );
        all.extend_from_slice(&logits);
        let is_answered = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            > logits.iter().cloned().fold(f32::INFINITY, f32::min);
        if is_answered {
            answered += 1;
            if argmax(&logits) == y[i] as usize {
                right += 1;
            }
        }
    }
    if let Some(path) = args.get(3) {
        let bytes: Vec<u8> = all.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(path, bytes).unwrap_or_else(|e| panic!("写 {path} 失败：{e}"));
    }
    let nf = n as f64;
    let accuracy = right as f64 / nf + (n - answered) as f64 / nf / 4.0;
    println!(
        "accuracy {accuracy:.16}｜answered {:.16}｜n={n}｜{:.2} ms",
        answered as f64 / nf,
        started.elapsed().as_secs_f64() * 1000.0
    );
}
