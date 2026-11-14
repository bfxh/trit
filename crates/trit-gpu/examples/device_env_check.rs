//! 跨环境检测：OS/架构/核数/SIMD 后端 + CPU 完整计划延迟 + NPU 模型值。
//!
//! 不碰 GPU，因此可在 CPU 容器 / WSL / 原生 Windows 直接跑；用于比较
//! 「同一份代码在不同环境里测到的 CPU 计划延迟与后端选择」。
//!
//! ```bash
//! cargo run --release -p trit-gpu --example device_env_check
//! ```

#![allow(dead_code)]

#[path = "device_adapt/cpu_measure.rs"]
mod cpu_measure;
#[path = "device_adapt/device.rs"]
mod device;
#[path = "device_adapt/fit.rs"]
mod fit;
#[path = "device_adapt/npu_sim.rs"]
mod npu_sim;
#[path = "device_adapt/plan.rs"]
mod plan;

use std::process::Command;

use serde_json::json;

use device::DeviceClass;
use plan::Shape;

const SHAPES: [Shape; 4] = [
    Shape::new(512, 4096, 512),
    Shape::new(768, 768, 768),
    Shape::new(320, 320, 320),
    Shape::new(64, 2048, 2048),
];

fn rustc_version() -> String {
    Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn main() {
    let cpus = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1);
    println!(
        "{}",
        json!({
            "stage": "env",
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cpus": cpus,
            "simd_backend": trit_simd::selected_backend().as_str(),
            "rustc": rustc_version(),
        })
    );
    let cpu = device::anchors()
        .into_iter()
        .find(|device| device.class == DeviceClass::Cpu)
        .expect("CPU 锚点存在");
    cpu_measure::run(&cpu, cpus, &SHAPES);
    npu_sim::run(&SHAPES);
    println!(
        "{}",
        json!({
            "stage": "final",
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "cpus": cpus,
            "simd_backend": trit_simd::selected_backend().as_str(),
            "verified": true,
        })
    );
}
