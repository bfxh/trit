//! 小形状校准：用实测延迟拟合设备的 `launch_us` 与 `occupancy`。
//!
//! 只调这两个**设备级**参数（计划效率仍由校准形状决定），目标是最小化
//! `|ln(model/measured)|` 的均值。默认留出最后一个形状做真实验证；小样本，
//! 结论按「拟合/留出同一台机器」读。

use crate::device::Device;
use crate::plan::{PLAN_COUNT, Plan, Shape};

#[derive(Clone, Debug)]
pub struct FitReport {
    pub launch_us: f64,
    pub occupancy: f64,
    pub fit_error: f64,
    pub holdout_before: f64,
    pub holdout_after: f64,
    pub holdout_shape: Shape,
}

fn mean_abs_log(
    device: &Device,
    plans: &[Plan],
    measured: &[(Shape, [Option<f64>; PLAN_COUNT])],
    indices: &[usize],
) -> f64 {
    let mut total = 0.0f64;
    let mut count = 0usize;
    for &index in indices {
        let (shape, latencies) = &measured[index];
        for (plan_index, plan) in plans.iter().enumerate() {
            if let Some(measured_latency) = latencies[plan_index]
                && let Some(model_latency) = plan.latency(device, *shape)
            {
                total += (model_latency / measured_latency).ln().abs();
                count += 1;
            }
        }
    }
    total / count.max(1) as f64
}

/// 在除最后一个形状外的样本上网格搜索；返回拟合值与留出误差。
pub fn fit_launch_occupancy(
    device: &mut Device,
    plans: &[Plan],
    measured: &[(Shape, [Option<f64>; PLAN_COUNT])],
) -> Option<FitReport> {
    if measured.len() < 2 {
        return None;
    }
    let holdout_index = measured.len() - 1;
    let fit_indices: Vec<usize> = (0..holdout_index).collect();
    let holdout = [holdout_index];
    let holdout_before = mean_abs_log(device, plans, measured, &holdout);

    let mut best = (device.launch_us, device.occupancy, f64::INFINITY);
    let launches = [0.5, 1.0, 2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0];
    let occupancies = [0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0];
    for &launch_us in &launches {
        for &occupancy in &occupancies {
            let mut candidate = device.clone();
            candidate.launch_us = launch_us;
            candidate.occupancy = occupancy;
            let error = mean_abs_log(&candidate, plans, measured, &fit_indices);
            if error < best.2 {
                best = (launch_us, occupancy, error);
            }
        }
    }
    device.launch_us = best.0;
    device.occupancy = best.1;
    let holdout_after = mean_abs_log(device, plans, measured, &holdout);
    Some(FitReport {
        launch_us: best.0,
        occupancy: best.1,
        fit_error: best.2,
        holdout_before,
        holdout_after,
        holdout_shape: measured[holdout_index].0,
    })
}
