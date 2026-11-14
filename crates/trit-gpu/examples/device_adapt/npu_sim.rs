//! NPU 的「完整计划延迟」目前只能是模型值：本机/本仓没有 NPU 后端。
//! 输出显式标 `measured=false`，不与 CPU/GPU 实测混淆。

use serde_json::json;

use crate::device::{DeviceClass, anchors};
use crate::plan::{PlanKind, Shape, plans};

pub fn run(shapes: &[Shape]) {
    println!(
        "{}",
        json!({
            "stage": "npu_available",
            "available": false,
            "why": "本机/本仓没有 NPU 后端；以下为模型值，不是实测",
        })
    );
    let plans = plans();
    let plan = plans
        .iter()
        .find(|plan| plan.kind == PlanKind::NpuInt4)
        .expect("NPU 计划存在");
    for device in anchors()
        .into_iter()
        .filter(|device| device.class == DeviceClass::Npu)
    {
        for &shape in shapes {
            let latency = plan.latency(&device, shape).expect("NPU 计划可行");
            println!(
                "{}",
                json!({
                    "stage": "npu_plan",
                    "measured": false,
                    "device": device.name,
                    "shape": shape.label(),
                    "plan": plan.kind.name(),
                    "model_ms": (latency * 1e6).round() / 1000.0,
                })
            );
        }
    }
}
