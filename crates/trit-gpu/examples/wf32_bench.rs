//! 生成器四层形状上的「三值权重 × f32 激活」GPU vs CPU 对比。
//!
//! 两种口径分开看：
//! - 端到端：含上传与回读，`run*` / `plan.run` 的墙钟；
//! - 内核段：`plan.run_timed` 里夹在 submit + poll 之间的时间，不含建缓冲、不含回读。
//!
//! 运行：
//!   cargo run -p trit-gpu --features gpu --release --example wf32_bench

use std::time::Instant;

use trit_gpu::matmul_wf32::Wf32Gpu;
use trit_gpu::matmul_wf32_plan::{Wf32Kernel, Wf32Plan, run_batch, run_chain};

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

struct Layer {
    name: &'static str,
    m: usize,
    n: usize,
    k: usize,
    a: Vec<i32>,
    b: Vec<f32>,
    ap: Vec<u32>,
    an: Vec<u32>,
}

impl Layer {
    fn build(name: &'static str, m: usize, n: usize, k: usize) -> Self {
        let words = k.div_ceil(32);
        let a: Vec<i32> = (0..m * k).map(|i| ((i * 7 + 1) % 3) as i32 - 1).collect();
        let b: Vec<f32> = (0..n * words * 32)
            .map(|i| ((i * 13 % 17) as f32) / 17.0 - 0.5)
            .collect();
        let (ap, an) = pack_rows(&a, k);
        Self {
            name,
            m,
            n,
            k,
            a,
            b,
            ap,
            an,
        }
    }

    fn cpu_ms(&self) -> f64 {
        let t = Instant::now();
        let sink = cpu_wf32(self.m, self.n, self.k, &self.a, &self.b);
        let _ = sink;
        t.elapsed().as_secs_f64() * 1000.0
    }

    fn ps_per_mac(&self, ms: f64) -> f64 {
        ms * 1e9 / (self.m * self.n * self.k) as f64
    }
}

/// 单个持久计划：缓冲一次分配，随后每轮只换激活。
fn bench_plan(layer: &Layer, plan: &Wf32Plan<'_>, old_ms: f64, reps: usize) {
    let kernel = Wf32Kernel::pick(layer.m, layer.n);
    let _ = plan.run(Some(kernel));

    // 端到端（单 submit：内核 + 回读同一条 command buffer）
    let t = Instant::now();
    let mut sink = 0f32;
    for _ in 0..reps {
        plan.set_activations(&layer.b);
        sink += plan.run(Some(kernel))[0];
    }
    let e2e_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;

    // 内核段 / 回读段分开（两次 submit，各自 poll 到完成）
    let t = Instant::now();
    let (mut compute_ms, mut readback_ms) = (0f64, 0f64);
    for _ in 0..reps {
        let (out, phases) = plan.run_timed(Some(kernel));
        sink += out[0];
        compute_ms += phases.compute_ms;
        readback_ms += phases.readback_ms;
    }
    let split_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
    compute_ms /= reps as f64;
    readback_ms /= reps as f64;

    let t = Instant::now();
    for _ in 0..reps {
        plan.set_activations(&layer.b);
    }
    let upload_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;

    println!(
        "  {:<20} {:?}：端到端 {e2e_ms:.3} ms（{:.2}x 旧）| 内核段 {compute_ms:.3} + 回读段 {readback_ms:.3}（两次 submit 共 {split_ms:.3}）| 激活上传 {upload_ms:.3} | 内核 {:.2} / 端到端 {:.2} ps/MAC",
        layer.name,
        kernel,
        old_ms / e2e_ms,
        layer.ps_per_mac(compute_ms),
        layer.ps_per_mac(e2e_ms),
    );
    assert!(sink.is_finite(), "{} 结果非有限", layer.name);
}

/// 四层合批：全部塞进一个 submit vs 每层各自 submit。
fn bench_batch(plans: &[Wf32Plan<'_>], reps: usize) {
    let refs: Vec<&Wf32Plan<'_>> = plans.iter().collect();
    let mut sink = 0f32;

    let _ = plans.iter().map(|p| p.run(None)).collect::<Vec<_>>();
    let t = Instant::now();
    for _ in 0..reps {
        for p in plans {
            sink += p.run(None)[0];
        }
    }
    let solo_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;

    let _ = run_batch(&refs, None);
    let t = Instant::now();
    for _ in 0..reps {
        sink += run_batch(&refs, None)[0][0];
    }
    let batch_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;

    // 合批不能改结果：同一批数据，逐层跑与合批逐元素对齐。
    let per = plans.iter().map(|p| p.run(None)).collect::<Vec<_>>();
    for (idx, (got, want)) in run_batch(&refs, None).iter().zip(&per).enumerate() {
        let worst = got
            .iter()
            .zip(want)
            .map(|(g, w): (&f32, &f32)| (g - w).abs())
            .fold(0f32, f32::max);
        assert!(
            worst < 1e-5,
            "第 {idx} 层合批与逐层不一致，最大偏差 {worst}"
        );
    }

    println!(
        "  四层合批：逐层 submit {solo_ms:.3} ms | 单 submit {batch_ms:.3} ms | {:.2}x",
        solo_ms / batch_ms
    );
    assert!(sink.is_finite(), "合批结果非有限");
}

fn main() {
    let Some(gpu) = Wf32Gpu::new() else {
        eprintln!("没有可用 GPU adapter");
        return;
    };
    println!("layer shapes (m rows, n pixels, k cols)");

    let shapes = [
        ("conv1 256x64 k9216", 256usize, 64usize, 9216usize),
        ("conv2 128x256 k2304", 128, 256, 2304),
        ("conv3 64x1024 k576", 64, 1024, 576),
        ("fc 16384x1 k128", 16384, 1, 128),
    ];
    let layers: Vec<Layer> = shapes
        .into_iter()
        .map(|(name, m, n, k)| Layer::build(name, m, n, k))
        .collect();

    println!("—— 旧口径：每次 run 新建缓冲 + 两次 submit ——");
    let mut old_ms = Vec::with_capacity(layers.len());
    for layer in &layers {
        let _ = gpu.run_auto(layer.m, layer.n, layer.k, &layer.ap, &layer.an, &layer.b);
        let t = Instant::now();
        let reps = 10;
        let mut sink = 0f32;
        for _ in 0..reps {
            sink += gpu.run_auto(layer.m, layer.n, layer.k, &layer.ap, &layer.an, &layer.b)[0];
        }
        let gpu_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        old_ms.push(gpu_ms);
        let cpu_ms = layer.cpu_ms();
        println!(
            "  {:<20} GPU {gpu_ms:.3} ms | CPU {cpu_ms:.1} ms | {:.2}x CPU | {:.2} ps/MAC | sink {:.3}",
            layer.name,
            cpu_ms / gpu_ms,
            layer.ps_per_mac(gpu_ms),
            sink
        );
    }

    println!("—— 持久缓冲：形状只分配一次 ——");
    let plans: Vec<Wf32Plan<'_>> = layers
        .iter()
        .map(|layer| gpu.plan(layer.m, layer.n, layer.k, &layer.ap, &layer.an, &layer.b))
        .collect();
    for (layer, (plan, old_ms)) in layers.iter().zip(plans.iter().zip(&old_ms)) {
        bench_plan(layer, plan, *old_ms, 10);
    }

    bench_batch(&plans, 10);

    println!("—— 层间不出显存（链式，1×1 栈）——");
    bench_chain(&chain_stack(), 10);
}

/// 行主序 `m×n` → 列主序补齐 `n×words*32`（链式的 host 侧对照）。
fn transpose_into_b(prev: &[f32], prev_m: usize, prev_n: usize) -> Vec<f32> {
    let words = prev_m.div_ceil(32);
    let mut b = vec![0f32; prev_n * words * 32];
    for c in 0..prev_n {
        for r in 0..prev_m {
            b[c * words * 32 + r] = prev[r * prev_n + c];
        }
    }
    b
}

/// 一条通道递减、像素不变的 1×1 栈：第一层吃 conv1 的形状，后两层 k 接上一层的 m。
fn chain_stack() -> Vec<Layer> {
    [
        ("L1 256x64 k9216", 256usize, 64usize, 9216usize),
        ("L2 128x64 k256", 128, 64, 256),
        ("L3 64x64 k128", 64, 64, 128),
    ]
    .into_iter()
    .map(|(name, m, n, k)| Layer::build(name, m, n, k))
    .collect()
}

/// 逐层（每层上传 + 回读 + host 侧重排）vs 链式（只首层上传、只末层回读，一个 submit）。
fn bench_chain(layers: &[Layer], reps: usize) {
    let Some(gpu) = Wf32Gpu::new() else { return };
    let plans: Vec<Wf32Plan<'_>> = layers
        .iter()
        .map(|l| gpu.plan(l.m, l.n, l.k, &l.ap, &l.an, &l.b))
        .collect();

    let mut sink = 0f32;
    let t = Instant::now();
    for _ in 0..reps {
        let mut b_in = layers[0].b.clone();
        for (idx, plan) in plans.iter().enumerate() {
            plan.set_activations(&b_in);
            let out = plan.run(None);
            sink += out[0];
            if idx + 1 < layers.len() {
                b_in = transpose_into_b(&out, layers[idx].m, layers[idx].n);
            }
        }
    }
    let per_layer_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;

    let refs: Vec<&Wf32Plan<'_>> = plans.iter().collect();
    let _ = run_chain(&refs, None);
    let t = Instant::now();
    let mut chained = Vec::new();
    for _ in 0..reps {
        chained = run_chain(&refs, None);
        sink += chained[0];
    }
    let chain_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;

    // 链式必须与逐层同解：中间层少跑的每一趟搬运都会被这条抓到。
    let mut b_in = layers[0].b.clone();
    let mut per_layer_out = Vec::new();
    for (idx, plan) in plans.iter().enumerate() {
        plan.set_activations(&b_in);
        let out = plan.run(None);
        per_layer_out.push(out.clone());
        if idx + 1 < layers.len() {
            b_in = transpose_into_b(&out, layers[idx].m, layers[idx].n);
        }
    }
    let worst = chained
        .iter()
        .zip(&per_layer_out[layers.len() - 1])
        .map(|(g, w): (&f32, &f32)| (g - w).abs())
        .fold(0f32, f32::max);
    assert!(worst < 1e-5, "链式与逐层不同解，最大偏差 {worst}");

    println!(
        "  三层 1×1 栈：逐层（{n_up} 上传 + {n_down} 回读）{per_layer_ms:.3} ms | 链式（1 上传 + 1 回读，单 submit）{chain_ms:.3} ms | {:.2}x | 偏差 {worst:.1e}",
        per_layer_ms / chain_ms,
        n_up = layers.len(),
        n_down = layers.len(),
    );
    assert!(sink.is_finite(), "链式结果非有限");
}
