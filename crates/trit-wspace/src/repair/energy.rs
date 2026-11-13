//! 裕量能量（从 `repair.rs` 拆出，控制单文件行数）。

use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

use super::softplus::softplus;

/// 裕量能量：即使符号还错，也奖励“更接近正确一侧”的状态。
pub(super) fn energy(net: &Net, oracle: &Oracle, keep: Option<&Oracle>) -> Result<f32> {
    let mut total = 0.0f32;
    for (input, want) in oracle.inputs().iter().zip(oracle.outputs()) {
        let as_f32: Vec<f32> = input.iter().map(|&v| f32::from(v.to_i8())).collect();
        let raw = net.readout(&as_f32)?;
        for (&value, &target) in raw.iter().zip(want) {
            total += match target {
                Trit::Pos => softplus(-value),
                Trit::Neg => softplus(value),
                Trit::Zero => softplus(value.abs()),
            };
        }
    }
    if let Some(keep) = keep {
        total += 10.0 * keep.loss(net)? as f32;
    }
    Ok(total)
}
