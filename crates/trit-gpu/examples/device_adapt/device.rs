//! 行为级设备档案：有效峰值、带宽、启动开销、占用率与并行单元。
//!
//! 单位：op/s（trit-MAC/s、int4-MAC/s 或 f32-MAC/s）、B/s、µs、B。
//! **不是周期级模型**：不模拟流水线/缓存行/指令发射，只回答「这个形状在这一档
//! 设备上跑这个计划大概多快」。
//!
//! 校准锚点：GPU 的 `gpu_rtx4060ti_anchor` 由 `trit-gpu` 真机读数反推
//! （512×4096×512：ternary reg2x2 **0.129 ps/MAC**、tiled 0.208、naive 1.60；
//! f32 tiled 2.641）。该锚点的 occupancy=1、launch=1µs：峰值本身就是实测
//! 最好内核的**有效吞吐**，真实占用/启动已折进去，避免二次记账。
//! 其余设备档案（含 CPU/NPU 锚点与所有采样设备）是**合成值**，不代表真实机器。

use trit_bench::SeedRng;

/// 计划支持的数值格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Precision {
    Ternary,
    Int4,
    F32,
}

/// 设备大类。
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeviceClass {
    Cpu,
    Gpu,
    Npu,
}

impl DeviceClass {
    pub fn all() -> [Self; 3] {
        [Self::Cpu, Self::Gpu, Self::Npu]
    }

    pub fn index(self) -> usize {
        match self {
            Self::Cpu => 0,
            Self::Gpu => 1,
            Self::Npu => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Npu => "npu",
        }
    }
}

/// 一份设备档案。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Device {
    pub name: String,
    pub class: DeviceClass,
    /// 有效三值吞吐（trit-MAC/s）；0 = 不支持。
    pub ternary_ops: f64,
    /// 有效 f32 吞吐（MAC/s）；0 = 不支持。
    pub f32_ops: f64,
    /// 有效 int4 吞吐（int4-MAC/s）；0 = 不支持。
    pub int4_ops: f64,
    /// 有效内存带宽（B/s）。
    pub bandwidth: f64,
    /// 可复用缓存/共享内存规模（B）。
    pub cache: f64,
    /// 单次内核启动的有效开销（µs）。
    pub launch_us: f64,
    /// 并行单元数（CPU 核 / GPU SM / NPU cluster）。
    pub units: f64,
    /// 占用率/效率（0-1）。
    pub occupancy: f64,
}

impl Device {
    pub fn peak(&self, precision: Precision) -> f64 {
        match precision {
            Precision::Ternary => self.ternary_ops,
            Precision::Int4 => self.int4_ops,
            Precision::F32 => self.f32_ops,
        }
    }

    pub fn supports(&self, precision: Precision) -> bool {
        self.peak(precision) > 0.0
    }
}

/// 测试集里的固定锚点（含真机校准点 + 合成点）。
pub fn anchors() -> Vec<Device> {
    vec![
        Device {
            name: "cpu_x86_anchor".into(),
            class: DeviceClass::Cpu,
            ternary_ops: 1.6e11,
            f32_ops: 5.0e10,
            int4_ops: 0.0,
            bandwidth: 50e9,
            cache: 32e6,
            launch_us: 0.5,
            units: 16.0,
            occupancy: 0.9,
        },
        Device {
            name: "cpu_server_anchor(sim)".into(),
            class: DeviceClass::Cpu,
            ternary_ops: 3.2e11,
            f32_ops: 1.2e11,
            int4_ops: 0.0,
            bandwidth: 120e9,
            cache: 64e6,
            launch_us: 0.3,
            units: 64.0,
            occupancy: 0.9,
        },
        Device {
            name: "gpu_rtx4060ti_anchor".into(),
            class: DeviceClass::Gpu,
            // 1 / 0.129 ps = 7.752e12 trit-MAC/s（实测 reg2x2）
            ternary_ops: 1.0 / 0.129e-12,
            // 1 / 2.641 ps = 3.786e11 MAC/s（实测 f32 tiled）
            f32_ops: 1.0 / 2.641e-12,
            int4_ops: 0.0,
            bandwidth: 288e9,
            cache: 32e6,
            launch_us: 1.0,
            units: 34.0,
            occupancy: 1.0,
        },
        Device {
            name: "gpu_datacenter_anchor(sim)".into(),
            class: DeviceClass::Gpu,
            ternary_ops: 3.0e13,
            f32_ops: 2.0e12,
            int4_ops: 0.0,
            bandwidth: 1.5e12,
            cache: 64e6,
            launch_us: 5.0,
            units: 108.0,
            occupancy: 0.85,
        },
        Device {
            name: "gpu_weak_ternary_anchor(sim)".into(),
            class: DeviceClass::Gpu,
            ternary_ops: 2.0e11,
            f32_ops: 4.5e11,
            int4_ops: 0.0,
            bandwidth: 200e9,
            cache: 8e6,
            launch_us: 6.0,
            units: 20.0,
            occupancy: 0.7,
        },
        Device {
            name: "npu_edge_anchor(sim)".into(),
            class: DeviceClass::Npu,
            ternary_ops: 0.0,
            f32_ops: 0.0,
            int4_ops: 8.0e12,
            bandwidth: 60e9,
            cache: 8e6,
            launch_us: 12.0,
            units: 8.0,
            occupancy: 0.8,
        },
        Device {
            name: "npu_datacenter_anchor(sim)".into(),
            class: DeviceClass::Npu,
            ternary_ops: 0.0,
            f32_ops: 0.0,
            int4_ops: 4.0e13,
            bandwidth: 400e9,
            cache: 32e6,
            launch_us: 8.0,
            units: 32.0,
            occupancy: 0.9,
        },
    ]
}

/// `[0,1)` 均匀分布。
pub fn unit(rng: &mut SeedRng) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

fn log_uniform(rng: &mut SeedRng, low: f64, high: f64) -> f64 {
    low * (high / low).powf(unit(rng))
}

/// 合成一份设备档案；同一 `serial` + 同一 RNG 流可复算。
pub fn sample_device(class: DeviceClass, rng: &mut SeedRng, serial: usize) -> Device {
    let (mut ternary, mut f32_ops, mut int4) = (0.0, 0.0, 0.0);
    let (bw_low, bw_high, cache_low, cache_high) = match class {
        DeviceClass::Cpu => {
            ternary = log_uniform(rng, 4.0e10, 3.2e11);
            f32_ops = log_uniform(rng, 2.0e10, 1.2e11);
            (30e9, 120e9, 8e6, 64e6)
        }
        DeviceClass::Gpu => {
            // 15% 的 GPU 是「popcount 弱、f32 强」的老卡：让 f32 计划有获胜机会。
            let weak = unit(rng) < 0.15;
            ternary = if weak {
                log_uniform(rng, 1.0e11, 5.0e11)
            } else {
                log_uniform(rng, 1.0e12, 3.0e13)
            };
            f32_ops = log_uniform(rng, 1.0e11, 2.0e12);
            (100e9, 2.0e12, 8e6, 64e6)
        }
        DeviceClass::Npu => {
            int4 = log_uniform(rng, 1.0e12, 4.0e13);
            (20e9, 500e9, 4e6, 32e6)
        }
    };
    let launch_low = match class {
        DeviceClass::Cpu => 0.2,
        DeviceClass::Gpu => 2.0,
        DeviceClass::Npu => 3.0,
    };
    let launch_high = match class {
        DeviceClass::Cpu => 2.0,
        DeviceClass::Gpu => 10.0,
        DeviceClass::Npu => 20.0,
    };
    let (units_low, units_high) = match class {
        DeviceClass::Cpu => (4.0, 64.0),
        DeviceClass::Gpu => (16.0, 128.0),
        DeviceClass::Npu => (8.0, 64.0),
    };
    let (occ_low, occ_high) = match class {
        DeviceClass::Cpu => (0.7, 0.95),
        DeviceClass::Gpu => (0.5, 0.92),
        DeviceClass::Npu => (0.6, 0.95),
    };
    Device {
        name: format!("{}_sim_{serial}", class.name()),
        class,
        ternary_ops: ternary,
        f32_ops,
        int4_ops: int4,
        bandwidth: log_uniform(rng, bw_low, bw_high),
        cache: log_uniform(rng, cache_low, cache_high),
        launch_us: log_uniform(rng, launch_low, launch_high),
        units: log_uniform(rng, units_low, units_high),
        occupancy: occ_low + (occ_high - occ_low) * unit(rng),
    }
}

fn jitter(rng: &mut SeedRng, sigma: f64) -> f64 {
    ((2.0 * unit(rng) - 1.0) * sigma).exp()
}

/// 参数扰动：模拟「设备档案是估的/测偏了」。峰值/带宽/单元/占用率都按乘性抖动。
pub fn perturb(device: &Device, rng: &mut SeedRng, sigma: f64) -> Device {
    let peak = |value: f64, rng: &mut SeedRng| {
        if value > 0.0 {
            value * jitter(rng, sigma)
        } else {
            0.0
        }
    };
    Device {
        name: format!("{}~noise{sigma:.2}", device.name),
        class: device.class,
        ternary_ops: peak(device.ternary_ops, rng),
        f32_ops: peak(device.f32_ops, rng),
        int4_ops: peak(device.int4_ops, rng),
        bandwidth: peak(device.bandwidth, rng),
        cache: peak(device.cache, rng).max(1e6),
        launch_us: peak(device.launch_us, rng).max(0.01),
        units: peak(device.units, rng).max(1.0),
        occupancy: (device.occupancy * jitter(rng, sigma)).clamp(0.2, 1.0),
    }
}
