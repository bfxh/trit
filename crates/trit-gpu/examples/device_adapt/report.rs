//! 评估与报告：合成模型评估、实测延迟评估、最差样本。
//!
//! `device_adapt`（默认构建）与 `device_calibrate`（gpu feature）共用；
//! 这里不出现 wgpu/matmul 类型，只吃 `(Shape, [Option<f64>; PLAN_COUNT])`。

#![allow(dead_code)]

use serde_json::json;
use trit_bench::SeedRng;

use crate::device::{Device, perturb};
use crate::plan::{PLAN_COUNT, Plan, Shape, oracle};
use crate::policy::Chooser;

pub fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

pub fn perturb_devices(devices: &[Device], seed: u64, noise: f64) -> Vec<Device> {
    let mut rng = SeedRng::new(seed);
    devices
        .iter()
        .map(|device| perturb(device, &mut rng, noise))
        .collect()
}

pub fn plan_wins(devices: &[Device], shapes: &[Shape], plans: &[Plan]) -> [usize; PLAN_COUNT] {
    let mut counts = [0usize; PLAN_COUNT];
    for device in devices {
        for &shape in shapes {
            counts[oracle(plans, device, shape).0] += 1;
        }
    }
    counts
}

pub struct Metrics {
    pub mean: f64,
    pub median: f64,
    pub p90: f64,
    pub exact: f64,
    pub within10: f64,
    pub by_class: [f64; 3],
}

pub fn evaluate(
    chooser: &mut dyn Chooser,
    devices: &[Device],
    shapes: &[Shape],
    plans: &[Plan],
) -> Metrics {
    let mut ratios = Vec::with_capacity(devices.len() * shapes.len());
    let mut exact = 0usize;
    let mut within10 = 0usize;
    let mut class_sum = [0.0f64; 3];
    let mut class_count = [0usize; 3];
    for device in devices {
        for &shape in shapes {
            let (_, oracle_latency) = oracle(plans, device, shape);
            let chosen = chooser.choose(device, shape, plans);
            let chosen_latency = plans[chosen]
                .latency(device, shape)
                .unwrap_or(f64::INFINITY);
            let ratio = chosen_latency / oracle_latency;
            ratios.push(ratio);
            if ratio <= 1.0 + 1e-9 {
                exact += 1;
            }
            if ratio <= 1.10 {
                within10 += 1;
            }
            let class = device.class.index();
            class_sum[class] += ratio;
            class_count[class] += 1;
        }
    }
    let total = ratios.len().max(1) as f64;
    let mean = ratios.iter().sum::<f64>() / total;
    let mut sorted = ratios;
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("有限比值"));
    let median = sorted[sorted.len() / 2];
    let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
    Metrics {
        mean,
        median,
        p90,
        exact: exact as f64 / total,
        within10: within10 as f64 / total,
        by_class: [
            class_sum[0] / class_count[0].max(1) as f64,
            class_sum[1] / class_count[1].max(1) as f64,
            class_sum[2] / class_count[2].max(1) as f64,
        ],
    }
}

pub fn print_eval(policy_name: &str, condition: &str, metrics: &Metrics) {
    println!(
        "{}",
        json!({
            "stage": "eval",
            "policy": policy_name,
            "condition": condition,
            "mean_ratio": round3(metrics.mean),
            "median_ratio": round3(metrics.median),
            "p90_ratio": round3(metrics.p90),
            "exact": round3(metrics.exact),
            "within_10": round3(metrics.within10),
            "by_class": {
                "cpu": round3(metrics.by_class[0]),
                "gpu": round3(metrics.by_class[1]),
                "npu": round3(metrics.by_class[2]),
            },
        })
    );
}

pub fn print_worst(
    policy_name: &str,
    condition: &str,
    chooser: &mut dyn Chooser,
    devices: &[Device],
    shapes: &[Shape],
    plans: &[Plan],
    limit: usize,
) {
    let mut rows = Vec::new();
    for device in devices {
        for &shape in shapes {
            let (oracle_index, oracle_latency) = oracle(plans, device, shape);
            let chosen = chooser.choose(device, shape, plans);
            let chosen_latency = plans[chosen]
                .latency(device, shape)
                .unwrap_or(f64::INFINITY);
            rows.push((
                chosen_latency / oracle_latency,
                device.name.clone(),
                shape.label(),
                plans[chosen].kind.name().to_string(),
                plans[oracle_index].kind.name().to_string(),
            ));
        }
    }
    rows.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("有限比值"));
    for (ratio, device, shape, chosen, best) in rows.into_iter().take(limit) {
        println!(
            "{}",
            json!({
                "stage": "worst",
                "policy": policy_name,
                "condition": condition,
                "ratio": round3(ratio),
                "device": device,
                "shape": shape,
                "chosen": chosen,
                "oracle": best,
            })
        );
    }
}

pub fn best_of(latencies: &[Option<f64>; PLAN_COUNT]) -> (usize, f64) {
    let mut best = (usize::MAX, f64::INFINITY);
    for (index, latency) in latencies.iter().enumerate() {
        if let Some(value) = latency
            && *value < best.1
        {
            best = (index, *value);
        }
    }
    assert!(best.0 != usize::MAX, "至少一个计划有实测延迟");
    best
}

pub struct MeasuredOracle<'a> {
    pub measured: &'a [(Shape, [Option<f64>; PLAN_COUNT])],
}

impl Chooser for MeasuredOracle<'_> {
    fn choose(&mut self, _device: &Device, shape: Shape, _plans: &[Plan]) -> usize {
        let latencies = &self
            .measured
            .iter()
            .find(|(candidate, _)| *candidate == shape)
            .expect("oracle 查找已测形状")
            .1;
        best_of(latencies).0
    }
}

pub fn evaluate_measured(
    chooser: &mut dyn Chooser,
    device: &Device,
    plans: &[Plan],
    measured: &[(Shape, [Option<f64>; PLAN_COUNT])],
) -> Metrics {
    let mut ratios = Vec::with_capacity(measured.len());
    let mut exact = 0usize;
    let mut within10 = 0usize;
    for (shape, latencies) in measured {
        let (_, best) = best_of(latencies);
        let chosen = chooser.choose(device, *shape, plans);
        let chosen_latency = latencies[chosen].unwrap_or(f64::INFINITY);
        let ratio = chosen_latency / best;
        ratios.push(ratio);
        if ratio <= 1.0 + 1e-9 {
            exact += 1;
        }
        if ratio <= 1.10 {
            within10 += 1;
        }
    }
    let total = ratios.len().max(1) as f64;
    let mean = ratios.iter().sum::<f64>() / total;
    let mut sorted = ratios;
    sorted.sort_by(|a, b| a.partial_cmp(b).expect("有限比值"));
    let median = sorted[sorted.len() / 2];
    let p90 = sorted[(sorted.len() * 9 / 10).min(sorted.len() - 1)];
    Metrics {
        mean,
        median,
        p90,
        exact: exact as f64 / total,
        within10: within10 as f64 / total,
        by_class: [mean, mean, mean],
    }
}

pub fn print_shape_choice(
    policy_name: &str,
    plans: &[Plan],
    measured: &[(Shape, [Option<f64>; PLAN_COUNT])],
    mut choose: impl FnMut(Shape) -> usize,
) {
    for (shape, latencies) in measured {
        let (best_index, best_latency) = best_of(latencies);
        let chosen = choose(*shape);
        let chosen_latency = latencies[chosen].unwrap_or(f64::INFINITY);
        println!(
            "{}",
            json!({
                "stage": "real_shape",
                "policy": policy_name,
                "shape": shape.label(),
                "chosen": plans[chosen].kind.name(),
                "chosen_ms": round3(chosen_latency * 1e3),
                "best": plans[best_index].kind.name(),
                "best_ms": round3(best_latency * 1e3),
                "ratio": round3(chosen_latency / best_latency),
            })
        );
    }
}
