//! 真实 WSP1 权重（或合成 384×384）的 WF32 GPU vs CPU 对拍与计时。
//!
//! 运行：
//!   cargo run -p trit-gpu --features gpu --release --example wspace_gpu -- \
//!     --file model.wsp --inputs 64

use std::time::Instant;

use trit_gpu::matmul_wf32::Wf32Gpu;
use trit_gpu::pack_rows;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn next_f32(&mut self) -> f32 {
        (self.next_u64() % 2001) as f32 / 1000.0 - 1.0
    }
}

fn main() {
    let mut file = String::new();
    let mut inputs = 64usize;
    let mut seed = 7u64;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--file" => file = args.next().expect("file"),
            "--inputs" => inputs = args.next().expect("inputs").parse().expect("num"),
            "--seed" => seed = args.next().expect("seed").parse().expect("num"),
            other => panic!("unknown arg {other}"),
        }
    }
    let (m, k, trits) = if file.is_empty() {
        let (m, k) = (384usize, 384usize);
        let mut rng = Rng::new(seed);
        let trits = (0..m * k)
            .map(|_| match rng.next_u64() % 3 {
                0 => -1i32,
                1 => 0i32,
                _ => 1i32,
            })
            .collect();
        (m, k, trits)
    } else {
        let text = std::fs::read_to_string(&file).expect("wsp1");
        let net = trit_wspace::from_wsp(&text).expect("wsp1");
        let layer = &net.layers()[0];
        let (m, k) = (layer.rows(), layer.cols());
        let mut trits = Vec::with_capacity(m * k);
        for row in 0..m {
            for col in 0..k {
                trits.push(layer.trit(row, col).expect("trit").to_i8() as i32);
            }
        }
        (m, k, trits)
    };
    let n = inputs.max(1);
    let words = k.div_ceil(32);
    let (a_pos, a_neg) = pack_rows(&trits, k);
    let mut rng = Rng::new(seed ^ 0x9e37_79b9_7f4a_7c15);
    let mut b = vec![0.0f32; n * words * 32];
    for col in 0..n {
        for index in 0..k {
            b[col * words * 32 + index] = rng.next_f32();
        }
    }

    let start = Instant::now();
    let mut cpu = vec![0.0f32; m * n];
    for row in 0..m {
        for col in 0..n {
            let mut acc = 0.0f32;
            for index in 0..k {
                acc += trits[row * k + index] as f32 * b[col * words * 32 + index];
            }
            cpu[row * n + col] = acc;
        }
    }
    let cpu_ms = start.elapsed().as_secs_f64() * 1000.0;

    let Some(gpu) = Wf32Gpu::new() else {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "gpu": false, "m": m, "n": n, "k": k,
                "note": "没有可用的 wgpu adapter"
            }))
            .expect("json")
        );
        return;
    };
    let start = Instant::now();
    let out = gpu.run_auto(m, n, k, &a_pos, &a_neg, &b);
    let gpu_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut max_abs_error = 0.0f32;
    for (left, right) in out.iter().zip(&cpu) {
        max_abs_error = max_abs_error.max((left - right).abs());
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "gpu": true,
            "m": m,
            "n": n,
            "k": k,
            "max_abs_error": max_abs_error,
            "cpu_ms": cpu_ms,
            "gpu_ms": gpu_ms,
            "speedup": if gpu_ms > 0.0 { cpu_ms / gpu_ms } else { 0.0 },
        }))
        .expect("json")
    );
}
