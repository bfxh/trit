//! 设备适配策略：QAT 三值分类器 + 基线选择器。
//!
//! 输入 = 设备档案特征 + 形状特征；输出 = 8 个计划的分数，argmax 即选择。
//! 训练标签 = 行为级模拟器的 oracle 计划（同一 (设备, 形状) 下延迟最小者）。
//! 推理时对不可行计划做硬 mask（设备大类/精度支持），把「结构约束」留给规则，
//! 把「同类计划里的性能取舍」留给学习。
//!
//! 对照：oracle（穷举模拟）、按类静态最优、手写启发式、随机。

use trit_bench::SeedRng;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::cross_entropy;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};

use crate::device::{Device, DeviceClass, perturb, unit};
use crate::plan::{PLAN_COUNT, Plan, PlanKind, Shape, feasible_indices, index_of, oracle};

/// 特征维度（设备 8 + 大类 one-hot 3 + 形状/强度 7 + 设备比值 3）。
pub const FEATURES: usize = 21;

pub fn features(device: &Device, shape: Shape) -> Vec<f32> {
    let log = |value: f64, denom: f64| ((value.max(1.0)).log10() as f32) / denom as f32;
    let class = |wanted: DeviceClass| {
        if device.class == wanted { 1.0 } else { 0.0 }
    };
    let macs = shape.macs();
    let ternary_bytes = shape.k as f64 * shape.n as f64 * 0.25
        + shape.m as f64 * shape.k as f64 * 4.0
        + shape.m as f64 * shape.n as f64 * 4.0;
    let intensity = macs / ternary_bytes.max(1.0);
    let launch_share = (device.launch_us * 1e-6) / (macs / device.ternary_ops.max(1.0) + 1e-12);
    let launch_feature = (launch_share.max(1e-9).log10() as f32).clamp(-6.0, 2.0) / 6.0;
    // 显式比值：弱 popcount GPU 上 ternary/f32 的选择靠减法比靠 MLP 自己学除法稳。
    let ternary_f32 = (device.ternary_ops.max(1.0) / device.f32_ops.max(1.0)).log10() as f32 / 4.0;
    let ops_per_byte =
        (device.ternary_ops.max(1.0) / device.bandwidth.max(1.0)).log10() as f32 / 8.0;
    let cache_bytes = (device.cache.max(1.0) / ternary_bytes.max(1.0)).log10() as f32 / 6.0;
    vec![
        log(device.ternary_ops, 13.0),
        log(device.f32_ops, 13.0),
        log(device.int4_ops, 13.0),
        log(device.bandwidth, 12.0),
        log(device.cache, 8.0),
        ((device.launch_us.max(0.01)).log10() as f32).clamp(-2.0, 1.0),
        (device.units / 128.0) as f32,
        device.occupancy as f32,
        class(DeviceClass::Cpu),
        class(DeviceClass::Gpu),
        class(DeviceClass::Npu),
        (shape.m as f32).log2() / 12.0,
        (shape.n as f32).log2() / 12.0,
        (shape.k as f32).log2() / 12.0,
        (macs as f32).log10() / 12.0,
        (intensity as f32).log10() / 6.0,
        (ternary_bytes as f32).log10() / 12.0,
        launch_feature,
        ternary_f32.clamp(-2.0, 2.0),
        ops_per_byte.clamp(-2.0, 2.0),
        cache_bytes.clamp(-2.0, 2.0),
    ]
}

#[derive(Clone, Debug)]
pub struct Sample {
    pub features: Vec<f32>,
    pub label: usize,
}

fn make_layer(rows: usize, cols: usize, rng: &mut SeedRng) -> TernaryDense {
    let latent: Vec<Vec<f32>> = (0..rows)
        .map(|_| {
            (0..cols)
                .map(|_| (unit(rng) * 2.0 - 1.0) as f32 * 0.05)
                .collect()
        })
        .collect();
    let flat: Vec<f32> = latent.iter().flatten().copied().collect();
    let scale0 = |row: usize| {
        let mean = latent[row].iter().map(|w| w.abs()).sum::<f32>() / cols as f32;
        if mean > 1e-6 { mean } else { 1.0 }
    };
    TernaryDense::new(
        rows,
        cols,
        |row, col| flat[row * cols + col],
        scale0,
        Ste::Clipped { ratio: 1.0 },
    )
}

/// 训练/测试样本：特征 + oracle 计划标签。
pub fn build_samples(
    devices: &[Device],
    shapes: &[Shape],
    plans: &[Plan],
    noise: f64,
    seed: u64,
) -> Vec<Sample> {
    let mut rng = SeedRng::new(seed);
    let mut out = Vec::with_capacity(devices.len() * shapes.len());
    for device in devices {
        let device = if noise > 0.0 {
            perturb(device, &mut rng, noise)
        } else {
            device.clone()
        };
        for &shape in shapes {
            let (label, _) = oracle(plans, &device, shape);
            out.push(Sample {
                features: features(&device, shape),
                label,
            });
        }
    }
    out
}

fn argmax(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).expect("有限分数"))
        .map(|(index, _)| index)
        .expect("非空分数")
}

pub struct Policy {
    guard: TernaryDense,
    out: TernaryDense,
}

impl Policy {
    pub fn new(seed: u64, hidden: usize) -> Self {
        let mut rng = SeedRng::new(seed);
        let guard = make_layer(hidden, FEATURES, &mut rng);
        let out = make_layer(PLAN_COUNT, hidden, &mut rng);
        Self { guard, out }
    }

    fn logits(&mut self, features: &[f32]) -> Vec<f32> {
        let hidden = self.guard.forward(features);
        let activated: Vec<f32> = hidden
            .iter()
            .map(|&value| Activation::Tanh.forward(value))
            .collect();
        self.out.forward(&activated)
    }

    /// 推理：不可行计划 mask 成 -∞，argmax 选计划。
    pub fn choose(&mut self, device: &Device, shape: Shape, plans: &[Plan]) -> usize {
        let logits = self.logits(&features(device, shape));
        let mut best = usize::MAX;
        let mut best_score = f32::NEG_INFINITY;
        for (index, plan) in plans.iter().enumerate() {
            if plan.feasible(device) && logits[index] > best_score {
                best_score = logits[index];
                best = index;
            }
        }
        if best == usize::MAX {
            best = feasible_indices(plans, device)[0];
        }
        best
    }

    pub fn training_accuracy(&mut self, samples: &[Sample]) -> f32 {
        let mut correct = 0usize;
        for sample in samples {
            let logits = self.logits(&sample.features);
            if argmax(&logits) == sample.label {
                correct += 1;
            }
        }
        correct as f32 / samples.len().max(1) as f32
    }
}

/// 完整训练：逐样本 QAT（每步清零梯度），返回策略与训练集准确率。
pub fn train(samples: &[Sample], epochs: usize, hidden: usize, seed: u64) -> (Policy, f32) {
    assert!(!samples.is_empty(), "训练集不能为空");
    let mut policy = Policy::new(seed, hidden);
    let mut rng = SeedRng::new(seed ^ 0x5eed_5eed);
    let mut adam_gl = Adam::new(1e-2);
    let mut adam_gs = Adam::new(1e-2);
    let mut adam_ol = Adam::new(1e-2);
    let mut adam_os = Adam::new(1e-2);
    let mut order: Vec<usize> = (0..samples.len()).collect();
    for _ in 0..epochs {
        for index in (1..order.len()).rev() {
            let other = (rng.next_u64() as usize) % (index + 1);
            order.swap(index, other);
        }
        for &index in &order {
            let sample = &samples[index];
            policy.guard.zero_grad();
            policy.out.zero_grad();
            let hidden_values = policy.guard.forward(&sample.features);
            let activated: Vec<f32> = hidden_values
                .iter()
                .map(|&value| Activation::Tanh.forward(value))
                .collect();
            let logits = policy.out.forward(&activated);
            let (_loss, grad_logits) = cross_entropy(&logits, sample.label);
            let grad_activated = policy.out.backward(&grad_logits);
            policy.out.update(&mut adam_ol, &mut adam_os);
            let grad_hidden: Vec<f32> = grad_activated
                .iter()
                .zip(&activated)
                .map(|(&grad, &value)| grad * Activation::Tanh.backward(value))
                .collect();
            let _ = policy.guard.backward(&grad_hidden);
            policy.guard.update(&mut adam_gl, &mut adam_gs);
        }
    }
    let accuracy = policy.training_accuracy(samples);
    (policy, accuracy)
}

pub trait Chooser {
    fn choose(&mut self, device: &Device, shape: Shape, plans: &[Plan]) -> usize;
}

pub struct Oracle;

impl Chooser for Oracle {
    fn choose(&mut self, device: &Device, shape: Shape, plans: &[Plan]) -> usize {
        oracle(plans, device, shape).0
    }
}

pub struct RandomChooser {
    rng: SeedRng,
}

impl RandomChooser {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: SeedRng::new(seed),
        }
    }
}

impl Chooser for RandomChooser {
    fn choose(&mut self, device: &Device, _shape: Shape, plans: &[Plan]) -> usize {
        let feasible = feasible_indices(plans, device);
        let index = (self.rng.next_u64() as usize) % feasible.len();
        feasible[index]
    }
}

/// 每个设备大类一个静态最优计划（训练集平均比值最小）。
pub struct Fixed {
    pub per_class: [usize; 3],
}

impl Chooser for Fixed {
    fn choose(&mut self, device: &Device, _shape: Shape, plans: &[Plan]) -> usize {
        let index = self.per_class[device.class.index()];
        if plans[index].feasible(device) {
            index
        } else {
            feasible_indices(plans, device)[0]
        }
    }
}

pub fn fixed_by_class(devices: &[Device], shapes: &[Shape], plans: &[Plan]) -> [usize; 3] {
    let mut out = [0usize; 3];
    for class in DeviceClass::all() {
        let mut best = (usize::MAX, f64::INFINITY);
        for (index, plan) in plans.iter().enumerate() {
            if plan.class != class {
                continue;
            }
            let mut sum = 0.0f64;
            let mut count = 0usize;
            for device in devices.iter().filter(|device| device.class == class) {
                for &shape in shapes {
                    if let Some(latency) = plan.latency(device, shape) {
                        let (_, oracle_latency) = oracle(plans, device, shape);
                        sum += latency / oracle_latency;
                        count += 1;
                    }
                }
            }
            if count > 0 {
                let mean = sum / count as f64;
                if mean < best.1 {
                    best = (index, mean);
                }
            }
        }
        assert!(best.0 != usize::MAX, "每个设备大类至少有一个可行计划");
        out[class.index()] = best.0;
    }
    out
}

/// 手写规则：只用大类 + 形状，不看具体峰值/带宽。
pub struct Heuristic;

impl Chooser for Heuristic {
    fn choose(&mut self, device: &Device, shape: Shape, plans: &[Plan]) -> usize {
        let kind = match device.class {
            DeviceClass::Npu => PlanKind::NpuInt4,
            DeviceClass::Cpu => {
                if shape.macs() < 1.0e6 {
                    PlanKind::CpuU64
                } else {
                    PlanKind::CpuSimd
                }
            }
            DeviceClass::Gpu => {
                if device.ternary_ops < device.f32_ops * 0.6 {
                    PlanKind::GpuF32Tiled
                } else if shape.m.min(shape.n) <= 64 && shape.k >= 512 {
                    PlanKind::GpuSplitK
                } else if shape.macs() >= 1.0e9 {
                    PlanKind::GpuReg
                } else {
                    PlanKind::GpuTiled
                }
            }
        };
        let index = index_of(plans, kind);
        if plans[index].feasible(device) {
            index
        } else {
            feasible_indices(plans, device)[0]
        }
    }
}

pub struct Learned<'a> {
    pub policy: &'a mut Policy,
}

impl Chooser for Learned<'_> {
    fn choose(&mut self, device: &Device, shape: Shape, plans: &[Plan]) -> usize {
        self.policy.choose(device, shape, plans)
    }
}
