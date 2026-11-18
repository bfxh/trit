//! 计划（内核变体）与行为级延迟模型。
//!
//! 对一次 `m×n×k` 的三值矩阵乘，计划决定：
//! - 走 CPU / GPU / NPU 哪类单元（可行性由设备大类与精度支持决定）；
//! - 计算效率（相对设备有效峰值）；
//! - 分块/寄存器阻塞/分 K 带来的并行单元与内存流量；
//! - 启动次数与占用率折减。
//!
//! 延迟 = `max(计算, 内存) / (设备占用率 × 计划占用率 × 并行利用率) + 启动开销`。
//! 这是**行为级**近似：不模拟缓存行、流水线、指令发射；只保留对「选计划」
//! 有影响的量级关系。模型常数见 `device.rs` 的校准说明。

use crate::device::{Device, DeviceClass, Precision};

/// 一次矩阵乘的形状。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub m: usize,
    pub n: usize,
    pub k: usize,
}

impl Shape {
    pub const fn new(m: usize, n: usize, k: usize) -> Self {
        Self { m, n, k }
    }

    pub fn macs(self) -> f64 {
        self.m as f64 * self.n as f64 * self.k as f64
    }

    pub fn label(self) -> String {
        format!("{}x{}x{}", self.m, self.n, self.k)
    }
}

/// 计划族。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanKind {
    CpuU64,
    CpuSimd,
    GpuNaive,
    GpuTiled,
    GpuReg,
    GpuSplitK,
    NpuInt4,
    GpuF32Tiled,
}

pub const PLAN_COUNT: usize = 8;

impl PlanKind {
    pub fn all() -> [Self; PLAN_COUNT] {
        [
            Self::CpuU64,
            Self::CpuSimd,
            Self::GpuNaive,
            Self::GpuTiled,
            Self::GpuReg,
            Self::GpuSplitK,
            Self::NpuInt4,
            Self::GpuF32Tiled,
        ]
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::CpuU64 => "cpu_u64",
            Self::CpuSimd => "cpu_simd",
            Self::GpuNaive => "gpu_naive",
            Self::GpuTiled => "gpu_tiled",
            Self::GpuReg => "gpu_reg_2x2",
            Self::GpuSplitK => "gpu_split_k4",
            Self::NpuInt4 => "npu_int4",
            Self::GpuF32Tiled => "gpu_f32_tiled",
        }
    }
}

/// 一个可调度的计划。
#[derive(Clone, Debug)]
pub struct Plan {
    pub kind: PlanKind,
    pub class: DeviceClass,
    pub precision: Precision,
    /// 相对设备有效峰值的计算效率。
    pub efficiency: f64,
    /// 内存效率。
    pub mem_efficiency: f64,
    pub tile_m: f64,
    pub tile_n: f64,
    pub split_k: f64,
    pub launches: f64,
    pub occupancy_factor: f64,
    /// 并行单元相对设备 units 的放大倍数。
    pub capacity_scale: f64,
    pub launch_scale: f64,
    /// 是否按 tile 重复读权重/激活（false = 流式读一遍）。
    pub tiled: bool,
}

#[allow(clippy::too_many_arguments)]
fn base(
    kind: PlanKind,
    class: DeviceClass,
    precision: Precision,
    efficiency: f64,
    mem_efficiency: f64,
    tile: f64,
    split_k: f64,
    launches: f64,
    occupancy_factor: f64,
    capacity_scale: f64,
    launch_scale: f64,
    tiled: bool,
) -> Plan {
    Plan {
        kind,
        class,
        precision,
        efficiency,
        mem_efficiency,
        tile_m: tile,
        tile_n: tile,
        split_k,
        launches,
        occupancy_factor,
        capacity_scale,
        launch_scale,
        tiled,
    }
}

/// 全部计划（顺序固定 = 策略类别顺序）。
pub fn plans() -> Vec<Plan> {
    vec![
        base(
            PlanKind::CpuU64,
            DeviceClass::Cpu,
            Precision::Ternary,
            0.625,
            0.85,
            1.0,
            1.0,
            1.0,
            1.0,
            2.0,
            1.0,
            false,
        ),
        base(
            PlanKind::CpuSimd,
            DeviceClass::Cpu,
            Precision::Ternary,
            1.0,
            0.9,
            1.0,
            1.0,
            1.0,
            0.9,
            8.0,
            2.0,
            false,
        ),
        base(
            PlanKind::GpuNaive,
            DeviceClass::Gpu,
            Precision::Ternary,
            0.081,
            0.6,
            1.0,
            1.0,
            1.0,
            1.0,
            64.0,
            1.0,
            false,
        ),
        base(
            PlanKind::GpuTiled,
            DeviceClass::Gpu,
            Precision::Ternary,
            0.62,
            0.85,
            16.0,
            1.0,
            1.0,
            1.0,
            4.0,
            1.0,
            true,
        ),
        base(
            PlanKind::GpuReg,
            DeviceClass::Gpu,
            Precision::Ternary,
            1.0,
            0.8,
            32.0,
            1.0,
            1.0,
            1.0,
            4.0,
            1.0,
            true,
        ),
        base(
            PlanKind::GpuSplitK,
            DeviceClass::Gpu,
            Precision::Ternary,
            0.55,
            0.75,
            16.0,
            4.0,
            2.0,
            0.8,
            4.0,
            1.0,
            true,
        ),
        base(
            PlanKind::NpuInt4,
            DeviceClass::Npu,
            Precision::Int4,
            0.8,
            0.9,
            16.0,
            1.0,
            1.0,
            0.95,
            4.0,
            1.0,
            true,
        ),
        base(
            PlanKind::GpuF32Tiled,
            DeviceClass::Gpu,
            Precision::F32,
            1.0,
            0.85,
            16.0,
            1.0,
            1.0,
            1.0,
            4.0,
            1.0,
            true,
        ),
    ]
}

impl Plan {
    pub fn feasible(&self, device: &Device) -> bool {
        device.class == self.class && device.supports(self.precision)
    }

    fn weight_bytes_per_mac(&self) -> f64 {
        match self.precision {
            Precision::Ternary => 0.25,
            Precision::Int4 => 0.5,
            Precision::F32 => 4.0,
        }
    }

    fn act_bytes_per_element(&self) -> f64 {
        match self.precision {
            Precision::Ternary | Precision::F32 => 4.0,
            Precision::Int4 => 1.0,
        }
    }

    fn working_set(&self, shape: Shape) -> f64 {
        let weights = shape.k as f64 * shape.n as f64 * self.weight_bytes_per_mac();
        let acts = shape.m as f64 * shape.k as f64 * self.act_bytes_per_element();
        weights + acts
    }

    /// 行为级内存流量：工作集能进缓存时只读一遍；否则按 tile 重复读。
    fn traffic(&self, shape: Shape, device: &Device) -> f64 {
        let weights = shape.k as f64 * shape.n as f64 * self.weight_bytes_per_mac();
        let acts = shape.m as f64 * shape.k as f64 * self.act_bytes_per_element();
        let out = shape.m as f64 * shape.n as f64 * 4.0;
        let mut total = if self.tiled && self.working_set(shape) > device.cache {
            let weight_reads = (shape.m as f64 / self.tile_m).ceil().max(1.0);
            let act_reads = (shape.n as f64 / self.tile_n).ceil().max(1.0);
            weights * weight_reads + acts * act_reads + out
        } else {
            weights + acts + out
        };
        if self.split_k > 1.0 {
            total += shape.m as f64 * shape.n as f64 * 4.0 * self.split_k;
        }
        total
    }

    fn units(&self, shape: Shape) -> f64 {
        match self.kind {
            PlanKind::CpuU64 | PlanKind::CpuSimd | PlanKind::GpuNaive => {
                shape.m as f64 * shape.n as f64
            }
            PlanKind::GpuReg => {
                let m = (shape.m as f64 / 32.0).ceil().max(1.0);
                let n = (shape.n as f64 / 32.0).ceil().max(1.0);
                m * n
            }
            _ => {
                let m = (shape.m as f64 / self.tile_m).ceil().max(1.0);
                let n = (shape.n as f64 / self.tile_n).ceil().max(1.0);
                m * n * self.split_k
            }
        }
    }

    /// 行为级延迟（秒）；不可行返回 `None`。
    pub fn latency(&self, device: &Device, shape: Shape) -> Option<f64> {
        if !self.feasible(device) {
            return None;
        }
        let peak = device.peak(self.precision);
        let compute = shape.macs() / (peak * self.efficiency);
        let bandwidth = if self.working_set(shape) <= device.cache {
            device.bandwidth * 1.5
        } else {
            device.bandwidth
        };
        let memory = self.traffic(shape, device) / (bandwidth * self.mem_efficiency);
        let base = compute.max(memory);
        let capacity = (device.units * self.capacity_scale).max(1.0);
        let parallel = 0.5 + 0.5 * (self.units(shape) / capacity).min(1.0);
        let time = base / (device.occupancy * self.occupancy_factor * parallel);
        Some(time + self.launches * device.launch_us * self.launch_scale * 1e-6)
    }
}

/// 设备上可行的计划下标。
pub fn feasible_indices(plans: &[Plan], device: &Device) -> Vec<usize> {
    plans
        .iter()
        .enumerate()
        .filter(|(_, plan)| plan.feasible(device))
        .map(|(index, _)| index)
        .collect()
}

/// 穷举模拟得到的 oracle 计划与延迟。
pub fn oracle(plans: &[Plan], device: &Device, shape: Shape) -> (usize, f64) {
    let mut best = (usize::MAX, f64::INFINITY);
    for (index, plan) in plans.iter().enumerate() {
        if let Some(latency) = plan.latency(device, shape)
            && latency < best.1
        {
            best = (index, latency);
        }
    }
    assert!(best.0 != usize::MAX, "设备至少有一个可行计划");
    best
}

pub fn index_of(plans: &[Plan], kind: PlanKind) -> usize {
    plans
        .iter()
        .position(|plan| plan.kind == kind)
        .expect("计划表固定包含全部 kind")
}
