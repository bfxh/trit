//! 真实设备校准：用本机读数替换 `device_adapt` 的合成档案。
//!
//! 采集面：
//! - CPU：`trit-simd::popcount_words`（SIMD 三值吞吐）、`trit_gpu::dot_words`
//!   （u32 位并行镜像）、f32 点积、内存拷贝带宽、逻辑核数；
//! - GPU：`trit_gpu::matmul::run` 的真实内核（naive/tiled/reg2x2/split-K/f32 tiled）、
//!   `trit-capacity::probe_gpu` 的显存、wgpu 拷贝带宽、adapter 名与 SM 数查表；
//! - 未直接测的量（cache/启动开销/占用率）显式标为假设。
//!
//! 用途：
//! 1. 在**校准形状**（512×4096×512）上把计划效率反推为实测比值，生成 GPU
//!    设备档案（`--out profile.json`）；
//! 2. 在**留出形状**上比较「校准模型 vs 实测」与「合成锚点 vs 实测」；
//! 3. 用合成设备训练 QAT 三值策略（与 `device_adapt` 同口径），在**实测
//!    计划延迟**上做零 shot 评估——这是真机读数，不是模拟器自证。
//!
//! 诚实边界：只测本机一块 GPU + 少量形状；其余设备仍是合成档案；模型未在
//! CPU/NPU 真机上验证。没有 adapter 时只输出 CPU 档案并退出 1。
//!
//! ```bash
//! cargo run --release -p trit-gpu --features gpu --example device_calibrate -- \
//!   --seed 7 --out D:/KF/_scratch/device-profile.json
//! ```

#[path = "device_adapt/cpu_measure.rs"]
mod cpu_measure;
#[path = "device_adapt/device.rs"]
mod device;
#[path = "device_adapt/fit.rs"]
mod fit;
#[path = "device_adapt/measured.rs"]
mod measured;
#[path = "device_adapt/npu_sim.rs"]
mod npu_sim;
#[path = "device_adapt/plan.rs"]
mod plan;
#[path = "device_adapt/policy.rs"]
mod policy;
#[path = "device_adapt/report.rs"]
mod report;

use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;

use serde_json::json;
use trit_bench::{Bench, SeedRng};
use trit_gpu::matmul::{self, MatmulReport};

use device::{Device, DeviceClass};
use plan::{PLAN_COUNT, Plan, PlanKind, Shape};
use policy::{Chooser, Fixed, Heuristic, Learned, RandomChooser};
use report::{MeasuredOracle, evaluate_measured, print_eval, print_shape_choice};

const CAL_SHAPE: Shape = Shape::new(512, 4096, 512);
const VALID_SHAPES: [Shape; 3] = [
    Shape::new(768, 768, 768),
    Shape::new(320, 320, 320),
    Shape::new(64, 2048, 2048),
];

const TRAIN_SHAPES: [Shape; 8] = [
    Shape::new(64, 64, 64),
    Shape::new(128, 128, 128),
    Shape::new(256, 256, 256),
    Shape::new(384, 384, 384),
    Shape::new(512, 512, 512),
    Shape::new(1024, 1024, 1024),
    Shape::new(128, 1024, 1024),
    Shape::new(2048, 2048, 2048),
];

struct Args {
    seed: u64,
    train_per_class: usize,
    epochs: usize,
    hidden: usize,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        seed: 7,
        train_per_class: 48,
        epochs: 120,
        hidden: 32,
        out: None,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--seed" => args.seed = argv.next().expect("seed").parse().expect("seed"),
            "--train-per-class" => {
                args.train_per_class = argv
                    .next()
                    .expect("train-per-class")
                    .parse()
                    .expect("train-per-class");
            }
            "--epochs" => {
                args.epochs = argv.next().expect("epochs").parse().expect("epochs");
            }
            "--hidden" => {
                args.hidden = argv.next().expect("hidden").parse().expect("hidden");
            }
            "--out" => args.out = Some(argv.next().expect("out")),
            other => panic!("未知参数 {other}"),
        }
    }
    args
}

struct CpuCalibration {
    device: Device,
    simd_ops_single: f64,
    u64_ops_single: f64,
    f32_ops_single: f64,
    copy_bandwidth: f64,
    cores: usize,
    backend: &'static str,
}

fn measure_cpu() -> CpuCalibration {
    let cores = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    let words = 1usize << 20;
    let a: Vec<u64> = (0..words)
        .map(|index| (index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .collect();
    let b: Vec<u64> = a
        .iter()
        .map(|value| value.rotate_left(17) ^ 0x5EED)
        .collect();
    let mut bench = Bench::new(words).reps(8);
    bench.add("popcount", || trit_simd::popcount_words(&a));
    let popc_ns_per_word = bench.measure(5)[0].1.ns_per_unit(words);
    let popc_per_s = 1e9 / popc_ns_per_word;

    let trits: Vec<i32> = (0..(1usize << 20))
        .map(|index| (index % 3) as i32 - 1)
        .collect();
    let (a_pos, a_neg) = trit_gpu::pack_dual_rail(&trits);
    let (b_pos, b_neg) = trit_gpu::pack_dual_rail(&b.iter().map(|_| 1).collect::<Vec<i32>>());
    let mut bench = Bench::new(trits.len()).reps(8);
    bench.add("dot_words", || {
        trit_gpu::dot_words(&a_pos, &a_neg, &b_pos, &b_neg) as u64
    });
    let u64_ns_per_mac = bench.measure(5)[0].1.ns_per_unit(trits.len());
    let u64_ops_single = 1e9 / u64_ns_per_mac;

    let f32a: Vec<f32> = (0..(1usize << 20))
        .map(|index| ((index % 17) as f32 - 8.0) * 0.01)
        .collect();
    let f32b: Vec<f32> = (0..(1usize << 20))
        .map(|index| ((index % 13) as f32 - 6.0) * 0.02)
        .collect();
    let mut bench = Bench::new(f32a.len()).reps(4);
    bench.add("f32_dot", || {
        let mut sum = 0.0f32;
        for (left, right) in f32a.iter().zip(&f32b) {
            sum += left * right;
        }
        u64::from(black_box(sum).to_bits())
    });
    let f32_ns_per_mac = bench.measure(5)[0].1.ns_per_unit(f32a.len());
    let f32_ops_single = 1e9 / f32_ns_per_mac;

    let copy_bytes = 128 * 1024 * 1024;
    let source = vec![7u8; copy_bytes];
    let mut target = vec![0u8; copy_bytes];
    let mut bench = Bench::new(copy_bytes).reps(1);
    bench.add("copy", || {
        target.copy_from_slice(&source);
        target[0] as u64
    });
    let copy_measures = bench.measure(5);
    let copy_bandwidth = 2.0 * copy_bytes as f64 / copy_measures[0].1.median_ns * 1e9;

    let simd_ops_single = popc_per_s * 8.0; // 32 trit 块 = 4 popcount ⇒ 8 trit/popc
    let device = Device {
        name: format!("cpu_measured_{cores}c"),
        class: DeviceClass::Cpu,
        ternary_ops: simd_ops_single * cores as f64,
        f32_ops: f32_ops_single * cores as f64,
        int4_ops: 0.0,
        bandwidth: copy_bandwidth,
        cache: 32e6,
        launch_us: 0.5,
        units: cores as f64,
        occupancy: 0.9,
    };
    CpuCalibration {
        device,
        simd_ops_single,
        u64_ops_single,
        f32_ops_single,
        copy_bandwidth,
        cores,
        backend: trit_simd::selected_backend().as_str(),
    }
}

/// 设备内存拷贝带宽（读+写），用 wgpu 的 buffer copy 量；失败返回 None。
fn measure_gpu_bandwidth() -> Option<(f64, String)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .ok()?;
    let info = adapter.get_info();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: adapter.limits(),
        ..Default::default()
    }))
    .ok()?;
    let size = 128u64 * 1024 * 1024;
    let usage = wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
    let left = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bandwidth-left"),
        size,
        usage,
        mapped_at_creation: false,
    });
    let right = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bandwidth-right"),
        size,
        usage,
        mapped_at_creation: false,
    });
    let mut warmup = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    warmup.copy_buffer_to_buffer(&left, 0, &right, 0, size);
    queue.submit([warmup.finish()]);
    device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
    let copies = 8u32;
    let mut samples = Vec::with_capacity(3);
    for _ in 0..3 {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        for _ in 0..copies {
            encoder.copy_buffer_to_buffer(&left, 0, &right, 0, size);
        }
        queue.submit([encoder.finish()]);
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let elapsed = start.elapsed().as_secs_f64();
        samples.push(2.0 * size as f64 * copies as f64 / elapsed);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("有限带宽"));
    Some((
        samples[samples.len() / 2],
        format!("{} ({:?})", info.name, info.backend),
    ))
}

fn sm_count(adapter: &str) -> (f64, &'static str) {
    let name = adapter.to_ascii_lowercase();
    if name.contains("4060 ti") {
        (34.0, "lookup(RTX 4060 Ti)")
    } else if name.contains("4090") {
        (128.0, "lookup(RTX 4090)")
    } else if name.contains("3060") {
        (28.0, "lookup(RTX 3060)")
    } else {
        (32.0, "assumed(32)")
    }
}

fn measured_ns(report: &MatmulReport, kind: PlanKind) -> Option<f64> {
    let ns = match kind {
        PlanKind::CpuU64 | PlanKind::CpuSimd | PlanKind::NpuInt4 => return None,
        PlanKind::GpuNaive => report.ternary_ns_per_mac,
        PlanKind::GpuTiled => report.ternary_tiled_ns_per_mac,
        PlanKind::GpuReg => report.ternary_reg_ns_per_mac,
        PlanKind::GpuSplitK => report.ternary_splitk_ns_per_mac,
        PlanKind::GpuF32Tiled => report.f32_tiled_ns_per_mac,
    };
    (ns.is_finite() && ns > 0.0).then_some(ns)
}

fn plan_index(kind: PlanKind) -> usize {
    PlanKind::all()
        .iter()
        .position(|candidate| *candidate == kind)
        .expect("PlanKind::all 覆盖全部")
}

fn measured_latencies(shape: Shape, report: &MatmulReport) -> [Option<f64>; PLAN_COUNT] {
    let macs = shape.macs();
    let mut out = [None; PLAN_COUNT];
    for kind in PlanKind::all() {
        if let Some(ns) = measured_ns(report, kind) {
            out[plan_index(kind)] = Some(ns * macs * 1e-9);
        }
    }
    out
}

struct GpuCalibration {
    device: Device,
    plans: Vec<Plan>,
    reports: Vec<(Shape, MatmulReport)>,
    bandwidth: f64,
    bandwidth_source: String,
    sm_source: &'static str,
}

fn calibrate_gpu() -> Option<GpuCalibration> {
    let mut shapes = vec![CAL_SHAPE];
    shapes.extend(VALID_SHAPES);
    let mut reports = Vec::new();
    for shape in shapes {
        let report = matmul::run(shape.m, shape.k, shape.n)?;
        let matches = report.matches
            && report.tiled_matches
            && report.reg_matches
            && report.splitk_matches
            && report.f32_tiled_matches;
        println!(
            "{}",
            json!({
                "stage": "gpu_report",
                "shape": shape.label(),
                "adapter": report.adapter,
                "backend": report.backend,
                "matches": matches,
                "ternary_naive_ps": report.ternary_ns_per_mac * 1000.0,
                "ternary_tiled_ps": report.ternary_tiled_ns_per_mac * 1000.0,
                "ternary_reg2_ps": report.ternary_reg_ns_per_mac * 1000.0,
                "ternary_splitk_ps": report.ternary_splitk_ns_per_mac * 1000.0,
                "f32_tiled_ps": report.f32_tiled_ns_per_mac * 1000.0,
            })
        );
        if !matches {
            eprintln!("跳过不正确的结果：{}", shape.label());
            continue;
        }
        reports.push((Shape::new(report.m, report.n, report.k), report));
    }
    let (calibration_shape, calibration) = reports.first()?;
    assert_eq!(*calibration_shape, CAL_SHAPE);
    let best_ternary_ns = [
        calibration.ternary_ns_per_mac,
        calibration.ternary_tiled_ns_per_mac,
        calibration.ternary_reg_ns_per_mac,
        calibration.ternary_splitk_ns_per_mac,
    ]
    .into_iter()
    .filter(|value| value.is_finite() && *value > 0.0)
    .fold(f64::INFINITY, f64::min);
    assert!(best_ternary_ns.is_finite(), "没有可用的三值内核读数");

    let (bandwidth, bandwidth_source) =
        measure_gpu_bandwidth().unwrap_or((288e9, "assumed(RTX 4060 Ti spec)".into()));
    let (units, sm_source) = sm_count(&calibration.adapter);
    let mut device = Device {
        name: format!("{} / {}", calibration.adapter, calibration.backend),
        class: DeviceClass::Gpu,
        ternary_ops: 1e9 / best_ternary_ns,
        f32_ops: if calibration.f32_tiled_ns_per_mac.is_finite()
            && calibration.f32_tiled_ns_per_mac > 0.0
        {
            1e9 / calibration.f32_tiled_ns_per_mac
        } else {
            0.0
        },
        int4_ops: 0.0,
        bandwidth,
        cache: 32e6,
        launch_us: 1.0,
        units,
        occupancy: 1.0,
    };

    let mut calibrated = plan::plans();
    let mappings = [
        (PlanKind::GpuNaive, calibration.ternary_ns_per_mac),
        (PlanKind::GpuTiled, calibration.ternary_tiled_ns_per_mac),
        (PlanKind::GpuReg, calibration.ternary_reg_ns_per_mac),
        (PlanKind::GpuSplitK, calibration.ternary_splitk_ns_per_mac),
    ];
    for (kind, ns) in mappings {
        if let Some(target) = calibrated.iter_mut().find(|plan| plan.kind == kind) {
            target.efficiency = (best_ternary_ns / ns).clamp(0.01, 1.0);
        }
    }
    if let Some(target) = calibrated
        .iter_mut()
        .find(|plan| plan.kind == PlanKind::GpuF32Tiled)
    {
        target.efficiency = 1.0;
    }
    let measured: Vec<(Shape, [Option<f64>; PLAN_COUNT])> = reports
        .iter()
        .map(|(shape, report)| (*shape, measured_latencies(*shape, report)))
        .collect();
    if let Some(fit) = fit::fit_launch_occupancy(&mut device, &calibrated, &measured) {
        println!(
            "{}",
            json!({
                "stage": "fit",
                "launch_us": fit.launch_us,
                "occupancy": fit.occupancy,
                "fit_error": (fit.fit_error * 1000.0).round() / 1000.0,
                "holdout_before": (fit.holdout_before * 1000.0).round() / 1000.0,
                "holdout_after": (fit.holdout_after * 1000.0).round() / 1000.0,
                "holdout_shape": fit.holdout_shape.label(),
            })
        );
    }
    Some(GpuCalibration {
        device,
        plans: calibrated,
        reports,
        bandwidth,
        bandwidth_source,
        sm_source,
    })
}

fn print_fidelity(stage: &str, plans: &[Plan], device: &Device, reports: &[(Shape, MatmulReport)]) {
    for (index, (shape, report)) in reports.iter().enumerate() {
        let measured = measured_latencies(*shape, report);
        let mut ratios = BTreeMap::new();
        let mut model_best = (usize::MAX, f64::INFINITY);
        let mut measured_best = (usize::MAX, f64::INFINITY);
        for (plan_index, plan) in plans.iter().enumerate() {
            if let Some(measured_latency) = measured[plan_index]
                && let Some(model_latency) = plan.latency(device, *shape)
            {
                ratios.insert(plan.kind.name(), model_latency / measured_latency);
                if model_latency < model_best.1 {
                    model_best = (plan_index, model_latency);
                }
                if measured_latency < measured_best.1 {
                    measured_best = (plan_index, measured_latency);
                }
            }
        }
        let values: Vec<f64> = ratios.values().copied().collect();
        let mean = values.iter().sum::<f64>() / values.len().max(1) as f64;
        let max = values.iter().copied().fold(0.0f64, f64::max);
        println!(
            "{}",
            json!({
                "stage": stage,
                "set": if index == 0 { "calibration" } else { "validation" },
                "shape": shape.label(),
                "mean_ratio": (mean * 1000.0).round() / 1000.0,
                "max_ratio": (max * 1000.0).round() / 1000.0,
                "model_best": plans[model_best.0].kind.name(),
                "measured_best": plans[measured_best.0].kind.name(),
                "best_match": model_best.0 == measured_best.0,
                "plan_ratios": ratios,
            })
        );
    }
}

fn vram() -> serde_json::Value {
    match trit_capacity::probe_gpu() {
        trit_capacity::Probe::Known(memory) => json!({
            "known": true,
            "total_bytes": memory.total,
            "used_bytes": memory.used,
            "free_bytes": memory.free,
            "source": memory.source.as_str(),
        }),
        trit_capacity::Probe::Unknown(why) => json!({"known": false, "why": why}),
    }
}

fn main() {
    let args = parse_args();
    let cpu = measure_cpu();
    println!(
        "{}",
        json!({
            "stage": "cpu_profile",
            "device": &cpu.device,
            "backend": cpu.backend,
            "cores": cpu.cores,
            "simd_trit_ops_single": cpu.simd_ops_single,
            "u64_trit_ops_single": cpu.u64_ops_single,
            "f32_ops_single": cpu.f32_ops_single,
            "copy_bandwidth": cpu.copy_bandwidth,
            "assumed": {"cache": 32e6, "launch_us": 0.5, "occupancy": 0.9},
        })
    );

    let mut all_shapes = vec![CAL_SHAPE];
    all_shapes.extend(VALID_SHAPES);
    cpu_measure::run(&cpu.device, cpu.cores, &all_shapes);
    npu_sim::run(&all_shapes);

    let Some(gpu) = calibrate_gpu() else {
        println!(
            "{}",
            json!({"stage": "gpu_unavailable", "why": "没有可用 adapter 或内核失败"})
        );
        std::process::exit(1);
    };
    println!(
        "{}",
        json!({
            "stage": "gpu_profile",
            "device": &gpu.device,
            "vram": vram(),
            "bandwidth": gpu.bandwidth,
            "bandwidth_source": gpu.bandwidth_source,
            "sm_source": gpu.sm_source,
            "assumed": {"cache": 32e6, "launch_us": 1.0, "occupancy": 1.0},
            "calibration_shape": CAL_SHAPE.label(),
        })
    );

    print_fidelity("model_fidelity", &gpu.plans, &gpu.device, &gpu.reports);
    let synthetic = device::anchors()
        .into_iter()
        .find(|candidate| candidate.name == "gpu_rtx4060ti_anchor")
        .expect("RTX 合成锚点存在");
    print_fidelity(
        "synthetic_fidelity",
        &plan::plans(),
        &synthetic,
        &gpu.reports,
    );

    // 训练仍用合成设备（学习器不知道真机）；真机读数只出现在评估侧。
    let mut rng = SeedRng::new(args.seed ^ 0x1234_5678);
    let mut train_devices = Vec::new();
    for class in DeviceClass::all() {
        for serial in 0..args.train_per_class {
            train_devices.push(device::sample_device(class, &mut rng, serial));
        }
    }
    let default_plans = plan::plans();
    let samples = policy::build_samples(
        &train_devices,
        &TRAIN_SHAPES,
        &default_plans,
        0.0,
        args.seed ^ 0xA11C_E5EE,
    );
    let (mut learned, accuracy) =
        policy::train(&samples, args.epochs, args.hidden, args.seed ^ 0x100);
    let fixed_plans = policy::fixed_by_class(&train_devices, &TRAIN_SHAPES, &default_plans);
    println!(
        "{}",
        json!({
            "stage": "train",
            "policy": "learned_clean",
            "samples": samples.len(),
            "epochs": args.epochs,
            "accuracy": (accuracy * 1000.0).round() / 1000.0,
        })
    );

    let measured: Vec<(Shape, [Option<f64>; PLAN_COUNT])> = gpu
        .reports
        .iter()
        .map(|(shape, report)| (*shape, measured_latencies(*shape, report)))
        .collect();

    let mut chooser = RandomChooser::new(args.seed ^ 0x5151);
    print_eval(
        "random",
        "measured",
        &evaluate_measured(&mut chooser, &gpu.device, &default_plans, &measured),
    );
    let mut chooser = MeasuredOracle {
        measured: &measured,
    };
    print_eval(
        "oracle_measured",
        "measured",
        &evaluate_measured(&mut chooser, &gpu.device, &default_plans, &measured),
    );
    let mut chooser = policy::Oracle;
    print_eval(
        "model_oracle_default",
        "measured",
        &evaluate_measured(&mut chooser, &gpu.device, &default_plans, &measured),
    );
    let mut chooser = Heuristic;
    print_eval(
        "heuristic",
        "measured",
        &evaluate_measured(&mut chooser, &gpu.device, &default_plans, &measured),
    );
    let mut chooser = Fixed {
        per_class: fixed_plans,
    };
    print_eval(
        "class_fixed",
        "measured",
        &evaluate_measured(&mut chooser, &gpu.device, &default_plans, &measured),
    );
    {
        let mut chooser = Learned {
            policy: &mut learned,
        };
        print_eval(
            "learned_clean",
            "measured",
            &evaluate_measured(&mut chooser, &gpu.device, &default_plans, &measured),
        );
    }
    print_shape_choice("learned_clean", &default_plans, &measured, |shape| {
        learned.choose(&gpu.device, shape, &default_plans)
    });
    let mut heuristic = Heuristic;
    print_shape_choice("heuristic", &default_plans, &measured, |shape| {
        heuristic.choose(&gpu.device, shape, &default_plans)
    });

    if let Some(path) = args.out {
        let mut shapes = BTreeMap::new();
        for (shape, report) in &gpu.reports {
            let latencies = measured_latencies(*shape, report);
            let mut entries = BTreeMap::new();
            for (index, latency) in latencies.iter().enumerate() {
                if let Some(seconds) = latency {
                    entries.insert(gpu.plans[index].kind.name().to_string(), *seconds);
                }
            }
            shapes.insert(shape.label(), entries);
        }
        let mut sources = BTreeMap::new();
        sources.insert("bandwidth".to_string(), gpu.bandwidth_source.clone());
        sources.insert("sm".to_string(), gpu.sm_source.to_string());
        sources.insert("calibration_shape".to_string(), CAL_SHAPE.label());
        let profile = measured::MeasuredProfile {
            device: gpu.device.clone(),
            shapes,
            sources,
        };
        profile.save(&path);
        println!("{}", json!({"stage": "profile_written", "path": path}));
    }
    println!(
        "{}",
        json!({
            "stage": "final",
            "seed": args.seed,
            "train_devices": train_devices.len(),
            "train_samples": samples.len(),
            "train_accuracy": (accuracy * 1000.0).round() / 1000.0,
            "measured_shapes": measured.iter().map(|(shape, _)| shape.label()).collect::<Vec<_>>(),
            "gpu": gpu.device.name,
            "bandwidth": gpu.bandwidth,
            "bandwidth_source": gpu.bandwidth_source,
            "sm_source": gpu.sm_source,
        })
    );
}
