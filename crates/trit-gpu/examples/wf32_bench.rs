//! 生成器四层形状上的「三值权重 × f32 激活」GPU vs CPU 对比。
//!
//! 运行：
//!   cargo run -p trit-gpu --features gpu --release --example wf32_bench

use std::time::Instant;

use trit_gpu::matmul_wf32::Wf32Gpu;

fn pack_rows(vals: &[i32], k: usize) -> (Vec<u32>, Vec<u32>) {
    let words = k.div_ceil(32);
    let rows = vals.len() / k;
    let mut pos = vec![0u32; rows * words];
    let mut neg = vec![0u32; rows * words];
    for r in 0..rows {
        for c in 0..k {
            match vals[r * k + c] {
                1 => pos[r * words + c / 32] |= 1u32 << (c % 32),
                -1 => neg[r * words + c / 32] |= 1u32 << (c % 32),
                _ => {}
            }
        }
    }
    (pos, neg)
}

fn cpu_wf32(m: usize, n: usize, k: usize, a: &[i32], b: &[f32]) -> f32 {
    let words = k.div_ceil(32);
    let mut sink = 0f32;
    for r in 0..m {
        for c in 0..n {
            let base = c * words * 32;
            let mut acc = 0f32;
            for kk in 0..k {
                acc += a[r * k + kk] as f32 * b[base + kk];
            }
            sink += acc;
        }
    }
    sink
}

fn main() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("没有可用 GPU adapter");
        return;
    };
    println!("layer shapes (m rows, n pixels, k cols); GPU run 含上传/读回");
    let shapes = [
        ("conv1 256x64 k9216", 256usize, 64usize, 9216usize),
        ("conv2 128x256 k2304", 128, 256, 2304),
        ("conv3 64x1024 k576", 64, 1024, 576),
        ("fc 16384x1 k128", 16384, 1, 128),
    ];
    for (name, m, n, k) in shapes {
        let words = k.div_ceil(32);
        let a: Vec<i32> = (0..m * k).map(|i| ((i * 7 + 1) % 3) as i32 - 1).collect();
        let b: Vec<f32> = (0..n * words * 32)
            .map(|i| ((i * 13 % 17) as f32) / 17.0 - 0.5)
            .collect();
        let (ap, an) = pack_rows(&a, k);

        let _ = gpu.run(m, n, k, &ap, &an, &b);
        let t0 = Instant::now();
        let reps = 10;
        let mut sink = 0f32;
        for _ in 0..reps {
            sink += gpu.run(m, n, k, &ap, &an, &b)[0];
        }
        let gpu_ms = t0.elapsed().as_secs_f64() * 1000.0 / reps as f64;

        let t1 = Instant::now();
        let csink = cpu_wf32(m, n, k, &a, &b);
        let cpu_ms = t1.elapsed().as_secs_f64() * 1000.0;

        let macs = (m * n * k) as f64;
        println!(
            "{name}: GPU {gpu_ms:.3} ms ({:.2} G ns/MAC) | CPU {cpu_ms:.1} ms | {:.2}x | sink {:.3}/{:.3}",
            gpu_ms * 1e6 / macs,
            cpu_ms / gpu_ms,
            sink,
            csink
        );
    }
}
