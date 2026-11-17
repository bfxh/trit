//! 复现《Weight-Space Programming Doesn't Need Transformers》里的二阶联想记忆读取器
//! （`models/<writer>/mem_assoc2*/arch.py` 的 `forward`）。
//!
//! 权重由 Python 侧的 `python/export_baseline.py` 从论文仓库的 `build.py` 导出为 npz；
//! 本程序只做 forward + ARC test 评测，用来验证 Rust 实现与论文读数逐题一致。
//!
//! 运行：
//! ```text
//! cargo run --release -- <weights.npz> <arc_test.npz>
//! ```

use std::path::Path;

use npyz::npz::NpzArchive;

mod quant;
mod weights;

use quant::{Quant, quantize_matrix};
use weights::{Weights, load_weights, read_i64};

const J: usize = 16;
const PRIME: i64 = 1_000_003;
const Q_LEN: usize = 128;
const O_LEN: usize = 48;

/// 一题的 forward：返回四个选项的分数。
fn forward(w: &Weights, q: &[i64], o: &[i64]) -> [f32; 4] {
    // 问题概念袋：一个概念出现一次即为 idf（scatter_ 置 1，不累加）。
    let mut qb = vec![0f32; w.c];
    for &id in q {
        let c = w.concept[id as usize];
        if c > 0 {
            qb[c as usize] = w.idf[c as usize];
        }
    }

    // top-J：值降序，值相等按概念 id 升序（与 torch.topk 的常见实现一致）。
    let mut top: Vec<usize> = (1..w.c).collect();
    top.sort_by(|&x, &y| qb[y].partial_cmp(&qb[x]).expect("有限值").then(x.cmp(&y)));
    top.truncate(J);

    // 二阶读取：120 个概念对各自读一行。
    let mut read = vec![0f32; w.c];
    for i in 0..top.len() {
        for j in (i + 1)..top.len() {
            let (x, y) = (top[i], top[j]);
            let strength = qb[x] * qb[y];
            if strength == 0.0 {
                continue;
            }
            let (lo, hi) = if x < y { (x, y) } else { (y, x) };
            let row = ((lo as i64 * PRIME + hi as i64) % w.h as i64) as usize;
            let base = row * w.r;
            for k in 0..w.r {
                let c = w.row_ids[base + k];
                if c >= 0 {
                    read[c as usize] += strength * w.row_w[base + k];
                }
            }
        }
    }

    // 一阶读取：只对 qb 非零的概念求和（等价于 qb @ A）。
    let mut acc = vec![0f32; w.c];
    for (d, &v) in qb.iter().enumerate().skip(1) {
        if v == 0.0 {
            continue;
        }
        let row = &w.a[d * w.c..(d + 1) * w.c];
        for c in 0..w.c {
            acc[c] += v * row[c];
        }
    }
    for c in 0..w.c {
        let masked = if qb[c] == 0.0 {
            read[c] + w.mix * acc[c]
        } else {
            0.0
        };
        read[c] = masked;
    }

    // 选项得分：read 覆盖了选项里多少概念（选项只保留问题没有的概念）。
    let mut logits = [0f32; 4];
    for (k, logit) in logits.iter_mut().enumerate() {
        let mut ob = vec![0f32; w.c];
        for &id in &o[k * O_LEN..(k + 1) * O_LEN] {
            let c = w.concept[id as usize];
            if c > 0 && qb[c as usize] == 0.0 {
                ob[c as usize] = w.idf[c as usize];
            }
        }
        *logit = read.iter().zip(ob.iter()).map(|(x, y)| x * y).sum::<f32>();
    }
    logits
}

fn argmax(scores: &[f32]) -> usize {
    let mut best = 0;
    for (i, v) in scores.iter().enumerate() {
        if *v > scores[best] {
            best = i;
        }
    }
    best
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!("用法：cargo run --release -- <weights.npz> <arc_test.npz> [logits.bin]");
        std::process::exit(2);
    }
    let mut w = load_weights(Path::new(&args[1]));
    let quant = Quant::from_env();
    let target = std::env::var("WSP_QUANT_TARGET").unwrap_or_else(|_| "both".to_string());
    if target == "a" || target == "both" {
        quantize_matrix(&mut w.a, w.c, w.c, quant);
    }
    if target == "row" || target == "both" {
        quantize_matrix(&mut w.row_w, w.h, w.r, quant);
    }
    println!(
        "量化 {:?}｜目标 {}｜A {}×{}｜二阶表 {}×{}",
        quant, target, w.c, w.c, w.h, w.r
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
        "accuracy {accuracy:.16}｜answered {:.16}｜a n={n}",
        answered as f64 / nf
    );
}
