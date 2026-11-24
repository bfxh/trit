//! `trit-train train-bench`：位平面 ALU 与三值点积的并发 / 预取剖析，附 STE 收敛对照。
//!
//! 用法：
//! ```text
//! train-bench [--mtrits 16] [--rounds 5] [--rows 4096] [--cols 576] [--threads 1,2,4,8] [--ste-steps 3000] [--train-steps 3] [--adam-scalar]
//! ```

use std::hint::black_box;
use std::time::Instant;
use trit_train::layer::{TernaryLinear, dot};
use trit_train::plane::{PlaneMatrix, Planes};
use trit_train::quant::Ste;

#[path = "bench/adam_step.rs"]
mod adam_step;
#[path = "bench/args.rs"]
mod args;
#[path = "bench/train_step.rs"]
mod train_step;
use args::{arg, thread_list};
use train_step::bench_train_step;

#[derive(Clone, Copy, Debug)]
enum Op {
    And,
    Or,
    Xor,
}

/// Kleene 语义的按字运算（与 `plane` 的公式一致；bench 只测这一套）。
fn apply(op: Op, ap: u64, an: u64, bp: u64, bn: u64) -> (u64, u64) {
    match op {
        Op::And => (ap & bp, an | bn),
        Op::Or => (ap | bp, an & bn),
        Op::Xor => {
            let pos = ap & bp;
            (pos, !(pos | !(ap | an) | !(bp | bn)))
        }
    }
}

fn sample_trits(len: usize, seed: u64) -> Vec<i8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 3) as i8 - 1
        })
        .collect()
}

fn bench_alu(a: &Planes, b: &Planes, op: Op, threads: usize, prefetch: bool, rounds: usize) -> f64 {
    let (a_pos, a_neg) = (a.pos_plane(), a.neg_plane());
    let (b_pos, b_neg) = (b.pos_plane(), b.neg_plane());
    let words = a.words();
    let mut out_pos = vec![0u64; words];
    let mut out_neg = vec![0u64; words];
    let mut best = f64::INFINITY;

    for _ in 0..rounds.max(1) {
        let start = Instant::now();
        let chunk = words.div_ceil(threads.max(1));
        std::thread::scope(|scope| {
            let mut pos_chunks = out_pos.chunks_mut(chunk);
            let mut neg_chunks = out_neg.chunks_mut(chunk);
            let mut base = 0usize;
            while let (Some(pos_out), Some(neg_out)) = (pos_chunks.next(), neg_chunks.next()) {
                let len = pos_out.len();
                let begin = base;
                let tap = prefetch;
                scope.spawn(move || {
                    for k in 0..len {
                        let i = begin + k;
                        if tap && i.is_multiple_of(8) {
                            let j = (i + 64).min(words - 1);
                            trit_train::prefetch::prefetch(&a_pos[j]);
                            trit_train::prefetch::prefetch(&a_neg[j]);
                            trit_train::prefetch::prefetch(&b_pos[j]);
                            trit_train::prefetch::prefetch(&b_neg[j]);
                        }
                        let (p, n) = apply(op, a_pos[i], a_neg[i], b_pos[i], b_neg[i]);
                        pos_out[k] = p;
                        neg_out[k] = n;
                    }
                });
                base += len;
            }
        });
        best = best.min(start.elapsed().as_secs_f64());
        black_box(&out_pos);
        black_box(&out_neg);
    }
    a.len() as f64 / best / 1e9
}

fn bench_dot(
    q: &[i8],
    rows: usize,
    cols: usize,
    x: &[f32],
    threads: usize,
    prefetch: bool,
    rounds: usize,
) -> f64 {
    let mut out = vec![0.0f32; rows];
    let mut best = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let start = Instant::now();
        let chunk = rows.div_ceil(threads.max(1));
        std::thread::scope(|scope| {
            for (tid, slot) in out.chunks_mut(chunk).enumerate() {
                let begin = tid * chunk;
                scope.spawn(move || {
                    for (k, value) in slot.iter_mut().enumerate() {
                        let r = begin + k;
                        *value = dot(&q[r * cols..(r + 1) * cols], x, prefetch);
                    }
                });
            }
        });
        best = best.min(start.elapsed().as_secs_f64());
        black_box(&out);
    }
    (rows * cols) as f64 / best / 1e9
}

/// 反向 `dX = Wᵀdy`：标量参考 vs 双导轨掩码内核，返回 `(标量, 内核, 加速比)`。
fn bench_backward(m: &PlaneMatrix, dy: &[f32], scale: &[f32], rounds: usize) -> (f64, f64, f64) {
    let macs = (m.rows() * m.cols()) as f64;
    let mut out = vec![0.0f32; m.cols()];
    let mut best_scalar = f64::INFINITY;
    let mut best_plane = f64::INFINITY;
    for _ in 0..rounds.max(1) {
        let start = Instant::now();
        m.input_grad_scalar(dy, scale, &mut out);
        best_scalar = best_scalar.min(start.elapsed().as_secs_f64());
        black_box(&out);
        let start = Instant::now();
        m.input_grad(dy, scale, &mut out);
        best_plane = best_plane.min(start.elapsed().as_secs_f64());
        black_box(&out);
    }
    let scalar = macs / best_scalar / 1e9;
    let plane = macs / best_plane / 1e9;
    (scalar, plane, plane / scalar)
}

/// 三值线性层在固定任务上的收敛（与 `layer` 的单测同一任务）。
fn ste_round(ste: Ste, steps: usize) -> (f32, Option<usize>, Vec<Vec<i8>>) {
    let (rows, cols) = (2usize, 3usize);
    let mut state = 0x1234_5678_9abc_def1u64;
    let mut layer = TernaryLinear::new(
        rows,
        cols,
        |_, _| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state % 100) as f32 - 50.0) * 0.001
        },
        ste,
        0.05,
        0.5,
    );
    let target = |r: usize, c: usize| match (r, c) {
        (0, 0) => 1.0,
        (0, 2) => -1.0,
        (1, 1) => 1.0,
        (1, 2) => 1.0,
        _ => 0.0,
    };
    let mut data = 0xdead_beef_0bad_f00du64;
    let mut loss = f32::NAN;
    let mut reached = None;
    for step in 0..steps {
        let x: Vec<f32> = (0..cols)
            .map(|_| {
                data ^= data << 13;
                data ^= data >> 7;
                data ^= data << 17;
                ((data % 2001) as f32 - 1000.0) / 1000.0
            })
            .collect();
        let y: Vec<f32> = (0..rows)
            .map(|r| (0..cols).map(|c| target(r, c) * x[c]).sum())
            .collect();
        loss = layer.train_step(&x, &y);
        if loss < 1e-4 && reached.is_none() {
            reached = Some(step + 1);
        }
    }
    (loss, reached, layer.quantized())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mtrits = arg(&args, "--mtrits").unwrap_or(16);
    let rounds = arg(&args, "--rounds").unwrap_or(5);
    let rows = arg(&args, "--rows").unwrap_or(4096);
    let cols = arg(&args, "--cols").unwrap_or(576);
    let ste_steps = arg(&args, "--ste-steps").unwrap_or(3000);
    let train_steps = arg(&args, "--train-steps").unwrap_or(3);
    let threads = thread_list(&args);
    let adam_scalar = args.iter().any(|a| a == "--adam-scalar");

    let trits = mtrits * 1_000_000; // 每 trit 2 bit（两条导轨各 1 bit）
    let a = Planes::from_trits(&sample_trits(trits, 0x9E37_79B9_7F4A_7C15));
    let b = Planes::from_trits(&sample_trits(trits, 0x0123_4567_89ab_cdef));
    println!(
        "位平面 ALU：{} M trit，Kleene 语义，{} 轮取最快",
        trits / 1_000_000,
        rounds
    );
    println!(
        "{:<5} {:>6} {:>10} {:>10} {:>10}",
        "线程", "预取", "AND Gop/s", "OR Gop/s", "XOR Gop/s"
    );
    for &t in &threads {
        for prefetch in [false, true] {
            let and = bench_alu(&a, &b, Op::And, t, prefetch, rounds);
            let or = bench_alu(&a, &b, Op::Or, t, prefetch, rounds);
            let xor = bench_alu(&a, &b, Op::Xor, t, prefetch, rounds);
            println!(
                "{t:<5} {:<6} {and:>10.2} {or:>10.2} {xor:>10.2}",
                if prefetch { "on" } else { "off" }
            );
        }
    }

    let q = sample_trits(rows * cols, 0xfeed_face_cafe_beef);
    let x: Vec<f32> = (0..cols).map(|i| (i % 29) as f32 * 0.03 - 0.4).collect();
    println!(
        "\n三值点积：{rows}×{cols}（{} M MAC），q ∈ {{-1,0,1}}",
        rows * cols / 1_000_000
    );
    println!("{:<5} {:<6} {:>10}", "线程", "预取", "GMAC/s");
    for &t in &threads {
        for prefetch in [false, true] {
            let gmac = bench_dot(&q, rows, cols, &x, t, prefetch, rounds);
            println!(
                "{t:<5} {:<6} {gmac:>10.2}",
                if prefetch { "on" } else { "off" }
            );
        }
    }

    let dy: Vec<f32> = (0..rows).map(|r| ((r % 31) as f32 - 15.0) / 15.0).collect();
    let scale: Vec<f32> = (0..rows).map(|r| 0.25 + ((r % 17) as f32) * 0.05).collect();
    let plane = PlaneMatrix::from_quantized(q, rows, cols);
    let (scalar, plane_gmac, speedup) = bench_backward(&plane, &dy, &scale, rounds);
    println!(
        "\n反向 dX：{rows}×{cols}（{} M MAC，单线程，{} 轮取最快）",
        rows * cols / 1_000_000,
        rounds
    );
    println!("{:<14} {:>10} {:>10}", "内核", "GMAC/s", "相对");
    println!("{:<14} {scalar:>10.2} {:>10}", "标量参考", "1.00×");
    println!("{:<14} {plane_gmac:>10.2} {:>9.2}×", "双导轨掩码", speedup);

    let (scalar_step, plane_step, step_speedup, pack_step) =
        bench_train_step(rows, cols, train_steps, rounds);
    println!("\n训练步：{rows}×{cols}（{train_steps} 步/轮；SGD fused，pack 另计）");
    println!("{:<14} {:>10} {:>10}", "路径", "ms/step", "相对");
    println!("{:<14} {scalar_step:>10.2} {:>10}", "标量参考", "1.00×");
    println!("{:<14} {plane_step:>10.2} {:>9.2}×", "融合步", step_speedup);
    println!("{:<14} {pack_step:>10.2}", "pack() 参考");
    adam_step::bench_adam_step(rows, cols, train_steps, rounds, adam_scalar);
    println!("\nSTE 收敛（2×3 线性层，{ste_steps} 步，MSE）：");
    for (name, ste) in [
        ("Identity", Ste::Identity),
        ("Clipped", Ste::Clipped { ratio: 1.0 }),
        ("Soft", Ste::Soft),
    ] {
        let (loss, reached, q) = ste_round(ste, ste_steps);
        let reached = reached.map_or_else(|| "未达标".to_string(), |s| format!("{s} 步"));
        println!("  {name:<9} loss={loss:.6}  达标(1e-4)={reached:<8} q={q:?}");
    }
}
