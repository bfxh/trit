//! 行为级硬件仿真 × 设备适配策略（CPU / GPU / NPU 计划选择）。
//!
//! **诚实口径**：这是行为级（非周期级）仿真 + 合成设备分布上的机制实验。
//! 仿真器的「真值」是它自己的闭式代价模型；GPU 锚点常数取自 `trit-gpu`
//! 真机读数（见 `device.rs`），其余设备是合成档案。**没有任何真机迁移结论**。
//!
//! 实验回答两件事：
//! 1. 计划选择任务是否有难度：oracle 计划是否随设备/形状变化，各计划是否
//!    都有获胜面；
//! 2. QAT 三值策略能否只凭设备档案 + 形状（不做逐设备 profiling）逼近
//!    oracle，并和「按类静态最优」「手写规则」「随机」比较；参数 ±noise
//!    抖动下是否仍稳。
//!
//! ```bash
//! cargo run --release -p trit-gpu --example device_adapt -- --epochs 120 --noise 0.10
//! ```
//!
//! 每行一个 JSON：`anchor` / `calibration` / `plan_wins` / `train` / `eval` / `final`。
//!
//! `--profile <json>`：消费 `device_calibrate` 的实测档案；用实测标签重训策略
//! （真机评估为 in-sample，合成留出另列）。

mod device;
mod measured;
mod plan;
mod policy;
mod profile_run;
mod report;

use serde_json::json;
use trit_bench::SeedRng;

use device::{Device, DeviceClass, anchors, sample_device};
use plan::{PLAN_COUNT, Plan, PlanKind, Shape, index_of, plans};
use policy::{Fixed, Heuristic, Learned, Oracle, RandomChooser};
use report::{evaluate, perturb_devices, plan_wins, print_eval, print_worst, round3};

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

const TEST_SHAPES: [Shape; 8] = [
    Shape::new(96, 96, 96),
    Shape::new(192, 192, 192),
    Shape::new(320, 320, 320),
    Shape::new(768, 768, 768),
    Shape::new(1536, 1536, 1536),
    Shape::new(64, 2048, 2048),
    Shape::new(2048, 2048, 64),
    Shape::new(4096, 4096, 4096),
];

struct Args {
    seed: u64,
    train_per_class: usize,
    test_per_class: usize,
    epochs: usize,
    hidden: usize,
    noise: f64,
    profile: Option<String>,
    profile_repeats: usize,
}

fn parse_args() -> Args {
    let mut args = Args {
        seed: 7,
        train_per_class: 48,
        test_per_class: 8,
        epochs: 120,
        hidden: 32,
        noise: 0.10,
        profile: None,
        profile_repeats: 256,
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
            "--test-per-class" => {
                args.test_per_class = argv
                    .next()
                    .expect("test-per-class")
                    .parse()
                    .expect("test-per-class");
            }
            "--epochs" => {
                args.epochs = argv.next().expect("epochs").parse().expect("epochs");
            }
            "--hidden" => {
                args.hidden = argv.next().expect("hidden").parse().expect("hidden");
            }
            "--noise" => {
                args.noise = argv.next().expect("noise").parse().expect("noise");
            }
            "--profile" => args.profile = Some(argv.next().expect("profile")),
            "--profile-repeats" => {
                args.profile_repeats = argv
                    .next()
                    .expect("profile-repeats")
                    .parse()
                    .expect("profile-repeats");
            }
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(args.train_per_class > 0 && args.test_per_class > 0);
    assert!(args.epochs > 0 && args.hidden > 0);
    assert!((0.0..1.0).contains(&args.noise));
    args
}

fn print_anchors() {
    for device in anchors() {
        println!(
            "{}",
            json!({
                "stage": "anchor",
                "name": device.name,
                "class": device.class.name(),
                "ternary_ops": device.ternary_ops,
                "f32_ops": device.f32_ops,
                "int4_ops": device.int4_ops,
                "bandwidth": device.bandwidth,
                "launch_us": device.launch_us,
                "units": device.units,
                "occupancy": round3(device.occupancy),
            })
        );
    }
}

/// 构造性一致性检查：锚点常数由这些真机读数反推，模型应复现它们。
fn print_calibration(plans: &[Plan]) {
    let anchor = anchors()
        .into_iter()
        .find(|device| device.name == "gpu_rtx4060ti_anchor")
        .expect("RTX 锚点存在");
    let shape = Shape::new(512, 4096, 512);
    let macs = shape.macs();
    let measured = [
        (PlanKind::GpuReg, 0.129),
        (PlanKind::GpuTiled, 0.208),
        (PlanKind::GpuNaive, 1.60),
        (PlanKind::GpuF32Tiled, 2.641),
    ];
    for (kind, ps_per_mac) in measured {
        let plan = &plans[index_of(plans, kind)];
        let model_us = plan.latency(&anchor, shape).expect("锚点计划可行") * 1e6;
        let measured_us = ps_per_mac * 1e-12 * macs * 1e6;
        println!(
            "{}",
            json!({
                "stage": "calibration",
                "plan": kind.name(),
                "shape": shape.label(),
                "model_us": round3(model_us),
                "measured_us": round3(measured_us),
                "ratio": round3(model_us / measured_us),
            })
        );
    }
}

fn main() {
    let args = parse_args();
    let plans = plans();
    print_anchors();
    print_calibration(&plans);

    let mut train_rng = SeedRng::new(args.seed ^ 0x1234_5678);
    let mut test_rng = SeedRng::new(args.seed ^ 0x9E37_79B9);
    let mut train_devices = Vec::new();
    for class in DeviceClass::all() {
        for serial in 0..args.train_per_class {
            train_devices.push(sample_device(class, &mut train_rng, serial));
        }
    }
    let measured_profile = args.profile.as_deref().map(measured::MeasuredProfile::load);
    let mut anchor_devices = anchors();
    if let Some(profile) = &measured_profile {
        println!(
            "{}",
            json!({
                "stage": "external_profile",
                "device": profile.device.name,
                "measured_shapes": profile.shapes.len(),
                "sources": profile.sources,
            })
        );
        anchor_devices.push(profile.device.clone());
    }
    let mut sampled_test_devices = Vec::new();
    for class in DeviceClass::all() {
        for serial in 0..args.test_per_class {
            sampled_test_devices.push(sample_device(class, &mut test_rng, 10_000 + serial));
        }
    }
    let test_devices: Vec<Device> = anchor_devices
        .iter()
        .chain(&sampled_test_devices)
        .cloned()
        .collect();

    let plan_names: Vec<&str> = PlanKind::all().iter().map(|kind| kind.name()).collect();
    let train_wins = plan_wins(&train_devices, &TRAIN_SHAPES, &plans);
    let test_wins = plan_wins(&test_devices, &TEST_SHAPES, &plans);
    println!(
        "{}",
        json!({"stage": "plan_wins", "set": "train", "counts": train_wins, "plans": plan_names})
    );
    println!(
        "{}",
        json!({"stage": "plan_wins", "set": "test", "counts": test_wins, "plans": plan_names})
    );

    let clean_samples = policy::build_samples(
        &train_devices,
        &TRAIN_SHAPES,
        &plans,
        0.0,
        args.seed ^ 0xA11C_E5EE,
    );
    let noisy_samples = policy::build_samples(
        &train_devices,
        &TRAIN_SHAPES,
        &plans,
        args.noise,
        args.seed ^ 0x0B0B_5EED,
    );
    let (mut clean_policy, clean_accuracy) =
        policy::train(&clean_samples, args.epochs, args.hidden, args.seed ^ 0x100);
    let (mut noisy_policy, noisy_accuracy) =
        policy::train(&noisy_samples, args.epochs, args.hidden, args.seed ^ 0x200);
    println!(
        "{}",
        json!({
            "stage": "train",
            "policy": "learned_clean",
            "samples": clean_samples.len(),
            "epochs": args.epochs,
            "hidden": args.hidden,
            "accuracy": round3(clean_accuracy as f64),
        })
    );
    println!(
        "{}",
        json!({
            "stage": "train",
            "policy": "learned_noisy",
            "samples": noisy_samples.len(),
            "epochs": args.epochs,
            "hidden": args.hidden,
            "noise": args.noise,
            "accuracy": round3(noisy_accuracy as f64),
        })
    );

    let fixed_plans = policy::fixed_by_class(&train_devices, &TRAIN_SHAPES, &plans);
    let noisy_anchors = perturb_devices(&anchor_devices, args.seed ^ 0x00C0_FFEE, args.noise);
    let noisy_sampled = perturb_devices(&sampled_test_devices, args.seed ^ 0x0BAD_5EED, args.noise);

    for (condition, devices) in [
        ("anchor_clean", &anchor_devices),
        ("sim_clean", &sampled_test_devices),
        ("anchor_noise", &noisy_anchors),
        ("sim_noise", &noisy_sampled),
    ] {
        let mut chooser = Oracle;
        print_eval(
            "oracle",
            condition,
            &evaluate(&mut chooser, devices, &TEST_SHAPES, &plans),
        );
        let mut chooser = Heuristic;
        print_eval(
            "heuristic",
            condition,
            &evaluate(&mut chooser, devices, &TEST_SHAPES, &plans),
        );
        let mut chooser = Fixed {
            per_class: fixed_plans,
        };
        print_eval(
            "class_fixed",
            condition,
            &evaluate(&mut chooser, devices, &TEST_SHAPES, &plans),
        );
        let mut chooser = RandomChooser::new(args.seed ^ 0x5151);
        print_eval(
            "random",
            condition,
            &evaluate(&mut chooser, devices, &TEST_SHAPES, &plans),
        );
        {
            let mut chooser = Learned {
                policy: &mut clean_policy,
            };
            print_eval(
                "learned_clean",
                condition,
                &evaluate(&mut chooser, devices, &TEST_SHAPES, &plans),
            );
        }
        {
            let mut chooser = Learned {
                policy: &mut clean_policy,
            };
            print_worst(
                "learned_clean",
                condition,
                &mut chooser,
                devices,
                &TEST_SHAPES,
                &plans,
                3,
            );
        }
        {
            let mut chooser = Learned {
                policy: &mut noisy_policy,
            };
            print_eval(
                "learned_noisy",
                condition,
                &evaluate(&mut chooser, devices, &TEST_SHAPES, &plans),
            );
        }
        {
            let mut chooser = Learned {
                policy: &mut noisy_policy,
            };
            print_worst(
                "learned_noisy",
                condition,
                &mut chooser,
                devices,
                &TEST_SHAPES,
                &plans,
                3,
            );
        }
    }

    if let Some(profile) = &measured_profile {
        profile_run::run(profile_run::Inputs {
            profile,
            plans: &plans,
            fixed_plans,
            train_shapes: &TRAIN_SHAPES,
            test_devices: &sampled_test_devices,
            clean_samples: &clean_samples,
            clean_policy: &mut clean_policy,
            seed: args.seed,
            epochs: args.epochs,
            hidden: args.hidden,
            repeats: args.profile_repeats,
        });
    }

    println!(
        "{}",
        json!({
            "stage": "final",
            "seed": args.seed,
            "train_devices": train_devices.len(),
            "anchor_devices": anchor_devices.len(),
            "sampled_test_devices": sampled_test_devices.len(),
            "test_devices": test_devices.len(),
            "train_samples": clean_samples.len(),
            "test_samples": test_devices.len() * TEST_SHAPES.len(),
            "plans": PLAN_COUNT,
            "epochs": args.epochs,
            "hidden": args.hidden,
            "noise": args.noise,
            "train_accuracy_clean": round3(clean_accuracy as f64),
            "train_accuracy_noisy": round3(noisy_accuracy as f64),
            "fixed_plans": fixed_plans.map(|index| plans[index].kind.name()),
        })
    );
}
