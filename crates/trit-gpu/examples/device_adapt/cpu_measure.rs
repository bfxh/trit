//! CPU 完整计划延迟：多线程 `cpu_u64`（dot_words）与 `cpu_simd`
//! （trit-simd popcount）在真实 matmul 形状上的墙钟时间。
//!
//! 只计内核循环，不含矩阵生成/打包；线程数 = `available_parallelism`。
//! 这是本机 CPU 的真实读数（与 GPU 的 MatmulReport 同构），不是模型值。

use std::hint::black_box;
use std::time::Instant;

use serde_json::json;
use trit_bench::SeedRng;

use crate::device::Device;
use crate::plan::{PLAN_COUNT, Plan, PlanKind, Shape};

const MAX_WORDS: usize = 256;

struct Inputs {
    a_pos: Vec<Vec<u32>>,
    a_neg: Vec<Vec<u32>>,
    b_pos: Vec<Vec<u32>>,
    b_neg: Vec<Vec<u32>>,
}

fn trits(len: usize, rng: &mut SeedRng) -> Vec<i32> {
    (0..len).map(|_| (rng.next_u64() % 3) as i32 - 1).collect()
}

fn prepare(shape: Shape, seed: u64) -> Inputs {
    let mut rng = SeedRng::new(seed);
    let mut a_pos = Vec::with_capacity(shape.m);
    let mut a_neg = Vec::with_capacity(shape.m);
    for _ in 0..shape.m {
        let row = trits(shape.k, &mut rng);
        let (pos, neg) = trit_gpu::pack_dual_rail(&row);
        a_pos.push(pos);
        a_neg.push(neg);
    }
    let mut b_pos = Vec::with_capacity(shape.n);
    let mut b_neg = Vec::with_capacity(shape.n);
    for _ in 0..shape.n {
        let column = trits(shape.k, &mut rng);
        let (pos, neg) = trit_gpu::pack_dual_rail(&column);
        b_pos.push(pos);
        b_neg.push(neg);
    }
    Inputs {
        a_pos,
        a_neg,
        b_pos,
        b_neg,
    }
}

fn dot_simd(a_pos: &[u32], a_neg: &[u32], b_pos: &[u32], b_neg: &[u32]) -> i64 {
    let words = a_pos.len();
    if words > MAX_WORDS {
        return trit_gpu::dot_words(a_pos, a_neg, b_pos, b_neg);
    }
    let mut pp = [0u64; MAX_WORDS];
    let mut pn = [0u64; MAX_WORDS];
    let mut np = [0u64; MAX_WORDS];
    let mut nn = [0u64; MAX_WORDS];
    for index in 0..words {
        pp[index] = u64::from(a_pos[index] & b_pos[index]);
        pn[index] = u64::from(a_pos[index] & b_neg[index]);
        np[index] = u64::from(a_neg[index] & b_pos[index]);
        nn[index] = u64::from(a_neg[index] & b_neg[index]);
    }
    let count = |buffer: &[u64]| trit_simd::popcount_words(&buffer[..words]) as i64;
    count(&pp) - count(&pn) - count(&np) + count(&nn)
}

fn run_once(shape: Shape, inputs: &Inputs, threads: usize, simd: bool) -> f64 {
    let start = Instant::now();
    let workers = threads.min(shape.m).max(1);
    let chunk = shape.m.div_ceil(workers);
    let sink = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for worker in 0..workers {
            let low = worker * chunk;
            let high = ((worker + 1) * chunk).min(shape.m);
            if low >= high {
                continue;
            }
            let a_pos = &inputs.a_pos;
            let a_neg = &inputs.a_neg;
            let b_pos = &inputs.b_pos;
            let b_neg = &inputs.b_neg;
            handles.push(scope.spawn(move || {
                let mut acc = 0i64;
                for row in low..high {
                    for column in 0..shape.n {
                        let value = if simd {
                            dot_simd(&a_pos[row], &a_neg[row], &b_pos[column], &b_neg[column])
                        } else {
                            trit_gpu::dot_words(
                                &a_pos[row],
                                &a_neg[row],
                                &b_pos[column],
                                &b_neg[column],
                            )
                        };
                        acc ^= value;
                    }
                }
                acc
            }));
        }
        let mut acc = 0i64;
        for handle in handles {
            acc ^= handle.join().expect("cpu worker");
        }
        acc
    });
    black_box(sink);
    start.elapsed().as_secs_f64()
}

pub struct Latency {
    pub shape: Shape,
    pub u64_s: f64,
    pub simd_s: f64,
}

fn measure(shapes: &[Shape], threads: usize) -> Vec<Latency> {
    let mut out = Vec::with_capacity(shapes.len());
    for &shape in shapes {
        let inputs = prepare(shape, 0x5EED_0000 ^ shape.macs() as u64);
        let mut u64_s = f64::INFINITY;
        let mut simd_s = f64::INFINITY;
        for _ in 0..2 {
            u64_s = u64_s.min(run_once(shape, &inputs, threads, false));
            simd_s = simd_s.min(run_once(shape, &inputs, threads, true));
        }
        out.push(Latency {
            shape,
            u64_s,
            simd_s,
        });
    }
    out
}

fn plan_index(plans: &[Plan], kind: PlanKind) -> usize {
    plans
        .iter()
        .position(|plan| plan.kind == kind)
        .expect("计划表包含 cpu 计划")
}

fn measured(latencies: &[Latency], plans: &[Plan]) -> Vec<(Shape, [Option<f64>; PLAN_COUNT])> {
    latencies
        .iter()
        .map(|row| {
            let mut values = [None; PLAN_COUNT];
            values[plan_index(plans, PlanKind::CpuU64)] = Some(row.u64_s);
            values[plan_index(plans, PlanKind::CpuSimd)] = Some(row.simd_s);
            (row.shape, values)
        })
        .collect()
}

fn print_fidelity(
    device: &Device,
    plans: &[Plan],
    measured: &[(Shape, [Option<f64>; PLAN_COUNT])],
) {
    for (shape, latencies) in measured {
        let mut ratios = Vec::new();
        for (index, plan) in plans.iter().enumerate() {
            if let Some(measured_latency) = latencies[index]
                && let Some(model_latency) = plan.latency(device, *shape)
            {
                ratios.push(model_latency / measured_latency);
            }
        }
        let mean = ratios.iter().sum::<f64>() / ratios.len().max(1) as f64;
        let max = ratios.iter().copied().fold(0.0f64, f64::max);
        println!(
            "{}",
            json!({
                "stage": "cpu_fidelity",
                "shape": shape.label(),
                "mean_ratio": (mean * 1000.0).round() / 1000.0,
                "max_ratio": (max * 1000.0).round() / 1000.0,
            })
        );
    }
}

pub fn run(device: &Device, threads: usize, shapes: &[Shape]) {
    let latencies = measure(shapes, threads);
    for row in &latencies {
        println!(
            "{}",
            json!({
                "stage": "cpu_full_latency",
                "shape": row.shape.label(),
                "cpu_u64_ms": (row.u64_s * 1e6).round() / 1000.0,
                "cpu_simd_ms": (row.simd_s * 1e6).round() / 1000.0,
                "threads": threads,
            })
        );
    }
    let calibration = &latencies[0];
    let macs = calibration.shape.macs();
    let u64_ops = macs / calibration.u64_s.max(1e-12);
    let simd_ops = macs / calibration.simd_s.max(1e-12);
    let best = u64_ops.max(simd_ops);
    let mut cpu = device.clone();
    cpu.ternary_ops = best;
    let mut plans = crate::plan::plans();
    for plan in &mut plans {
        match plan.kind {
            PlanKind::CpuU64 => plan.efficiency = (u64_ops / best).clamp(0.01, 1.0),
            PlanKind::CpuSimd => plan.efficiency = (simd_ops / best).clamp(0.01, 1.0),
            _ => {}
        }
    }
    let measured = measured(&latencies, &plans);
    if let Some(fit) = crate::fit::fit_launch_occupancy(&mut cpu, &plans, &measured) {
        println!(
            "{}",
            json!({
                "stage": "cpu_fit",
                "launch_us": fit.launch_us,
                "occupancy": fit.occupancy,
                "fit_error": (fit.fit_error * 1000.0).round() / 1000.0,
                "holdout_before": (fit.holdout_before * 1000.0).round() / 1000.0,
                "holdout_after": (fit.holdout_after * 1000.0).round() / 1000.0,
                "holdout_shape": fit.holdout_shape.label(),
            })
        );
    }
    let efficiencies: std::collections::BTreeMap<&str, f64> = plans
        .iter()
        .filter(|plan| matches!(plan.kind, PlanKind::CpuU64 | PlanKind::CpuSimd))
        .map(|plan| {
            (
                plan.kind.name(),
                (plan.efficiency * 1000.0).round() / 1000.0,
            )
        })
        .collect();
    println!(
        "{}",
        json!({
            "stage": "cpu_calibrated",
            "device": cpu,
            "efficiencies": efficiencies,
            "calibration_shape": calibration.shape.label(),
            "threads": threads,
            "u64_ops": u64_ops,
            "simd_ops": simd_ops,
        })
    );
    print_fidelity(&cpu, &plans, &measured);
}
