//! 实测档案格式：设备档案 + 形状→计划→延迟（秒）。
//!
//! `device_calibrate` 产出，`device_adapt --profile` 消费。兼容旧的纯
//! `Device` JSON（无实测延迟时退化成只带设备档案）。

#![allow(dead_code)]

use std::collections::BTreeMap;

use crate::device::Device;
use crate::plan::{PLAN_COUNT, Plan, Shape};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MeasuredProfile {
    pub device: Device,
    /// 形状标签（如 `512x4096x512`）→ 计划名 → 延迟（秒）。
    #[serde(default)]
    pub shapes: BTreeMap<String, BTreeMap<String, f64>>,
    /// 采集来源/假设（给人看，不参与计算）。
    #[serde(default)]
    pub sources: BTreeMap<String, String>,
}

impl MeasuredProfile {
    pub fn load(path: &str) -> Self {
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("读不到实测档案 {path}: {error}"));
        let value: serde_json::Value =
            serde_json::from_str(&text).unwrap_or_else(|error| panic!("档案不是 JSON: {error}"));
        if value.get("device").is_some() {
            serde_json::from_value(value).unwrap_or_else(|error| panic!("档案字段不匹配: {error}"))
        } else {
            let device: Device = serde_json::from_value(value)
                .unwrap_or_else(|error| panic!("档案字段不匹配: {error}"));
            Self {
                device,
                shapes: BTreeMap::new(),
                sources: BTreeMap::new(),
            }
        }
    }

    pub fn save(&self, path: &str) {
        let text = serde_json::to_string_pretty(self).expect("档案序列化");
        std::fs::write(path, text).unwrap_or_else(|error| panic!("写实测档案 {path}: {error}"));
    }

    /// 解析成评估用的 `(Shape, 每计划延迟)`；未知形状/计划名直接跳过。
    pub fn measured(&self, plans: &[Plan]) -> Vec<(Shape, [Option<f64>; PLAN_COUNT])> {
        let mut out = Vec::new();
        for (label, entries) in &self.shapes {
            let Some(shape) = parse_shape(label) else {
                continue;
            };
            let mut latencies = [None; PLAN_COUNT];
            for (plan_name, seconds) in entries {
                if let Some(index) = plans.iter().position(|plan| plan.kind.name() == plan_name) {
                    latencies[index] = Some(*seconds);
                }
            }
            if latencies.iter().any(Option::is_some) {
                out.push((shape, latencies));
            }
        }
        out
    }
}

pub fn parse_shape(label: &str) -> Option<Shape> {
    let mut parts = label.split('x');
    let m = parts.next()?.parse().ok()?;
    let n = parts.next()?.parse().ok()?;
    let k = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Shape::new(m, n, k))
}
