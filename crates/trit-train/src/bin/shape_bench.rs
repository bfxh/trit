//! 多形状训练步矩阵：SGD `train_step` 与 Adam `train_step_with_grad_adam`。
//! 计时含 `pack()`，构造与输入生成不计；每轮重建层与优化器，取最快。

use std::hint::black_box;
use std::time::Instant;

use trit_train::layer::{Adam, TernaryLinear as L};
use trit_train::quant::Ste;

#[rustfmt::skip]
const SHAPES: [(usize, usize); 7] = [(1, 1), (2, 7), (5, 9), (9, 65), (11, 130), (17, 576), (4096, 576)];

fn build(rows: usize, cols: usize) -> L {
    L::new(
        rows,
        cols,
        |r, c| (((r * 37 + c * 11 + 5) % 101) as f32 - 50.0) / 50.0,
        Ste::Soft,
        0.01,
        0.5,
    )
}

fn ramp(n: usize, mul: usize, add: usize, modulus: usize, denom: f32) -> Vec<f32> {
    (0..n)
        .map(|i| ((i * mul + add) % modulus) as f32 / denom - 1.0)
        .collect()
}

fn steps_for(rows: usize, cols: usize) -> usize {
    match rows * cols {
        0..=100 => 100_000,
        101..=10_000 => 5_000,
        _ => 3,
    }
}

fn b<S, R, F, G>(r: usize, c: usize, n: usize, k: usize, mut new: G, mut f: F) -> f64
where
    G: FnMut() -> S,
    F: FnMut(&mut L, &mut S) -> R,
{
    let mut best = f64::INFINITY;
    for _ in 0..k.max(1) {
        let mut layer = build(r, c);
        let mut state = new();
        let start = Instant::now();
        for _ in 0..n.max(1) {
            f(&mut layer, &mut state);
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    best * 1000.0 / n.max(1) as f64
}

fn main() {
    let k = 9;
    println!("多形状训练步矩阵（{k} 轮取最快；SGD 融合前向，Adam 含 pack）");
    println!(
        "{:<12} {:>10} {:>10} {:>8} {:>10} {:>10} {:>8} {:>7}",
        "shape", "SGD ms", "SGD 标量", "SGD×", "Adam ms", "Adam 标量", "Adam×", "步/轮"
    );
    let sf = L::train_step;
    let ss = L::train_step_scalar;
    let af = L::train_step_with_grad_adam;
    let ar = L::train_step_with_grad_adam_scalar;
    for (r, c) in SHAPES {
        let n = steps_for(r, c);
        let x = ramp(c, 29, 7, 67, 33.0);
        let dy = ramp(r, 17, 3, 41, 20.0);
        let t = ramp(r, 13, 9, 73, 36.0);
        let mk = || (Adam::new(r * c, 0.01), Adam::new(r, 0.01));
        let sgd = b(r, c, n, k, || (), |l, _| black_box(sf(l, &x, &t)));
        let sgd_ref = b(r, c, n, k, || (), |l, _| black_box(ss(l, &x, &t)));
        let adam = b(r, c, n, k, mk, |l, o| {
            black_box(af(l, &x, &dy, &mut o.0, &mut o.1))
        });
        let adam_ref = b(r, c, n, k, mk, |l, o| {
            black_box(ar(l, &x, &dy, &mut o.0, &mut o.1))
        });
        println!(
            "{:<12} {sgd:>10.4} {sgd_ref:>10.4} {:>7.2}× {adam:>10.4} {adam_ref:>10.4} {:>7.2}× {n:>7}",
            format!("{r}×{c}"),
            sgd_ref / sgd,
            adam_ref / adam
        );
    }
}
