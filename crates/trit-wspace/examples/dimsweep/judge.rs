//! dimsweep 的判据：线性 / tanh / two-tap 三种教师，以及降维 f32 对照。
//!
//! 从 `basis.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::{dot, teacher_forward};
use trit_wspace::safetensors::Weights;

/// 判据的形状。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Teacher {
    /// `sign(readout · (query·x))`：线性，`w_eff = queryᵀreadout` 是精确判据方向。
    Linear,
    /// `sign(readout · tanh(query·x))`：逐元素压缩后点积，`w_eff` 不再是精确方向。
    Tanh,
    /// `sign((r1·v)(r2·v))`：两条线性判据相乘（二次型），`w_eff` 不存在。
    TwoTap,
    /// `sign(readout · sign(query·x))`：判据本身带中间 `sign`，`w_eff` 不再是精确方向。
    DeepSign,
}

pub fn parse_teacher(name: &str) -> Teacher {
    match name {
        "linear" => Teacher::Linear,
        "tanh" => Teacher::Tanh,
        "two-tap" => Teacher::TwoTap,
        "deep-sign" => Teacher::DeepSign,
        other => panic!("未知 --teacher {other}（linear | tanh | two-tap）"),
    }
}

/// 逐元素三值符号（f32 版，用于把中间层压成 `sign`）。
pub fn sign_f32(value: f32) -> f32 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// 教师分数（未取符号）。
pub fn teacher_score(
    query: &Weights,
    readout: &[f32],
    second: Option<&[f32]>,
    x: &[f32],
    kind: Teacher,
) -> f32 {
    let v = teacher_forward(query, x);
    match kind {
        Teacher::Linear => dot(readout, &v),
        Teacher::Tanh => {
            let t: Vec<f32> = v.iter().map(|a| a.tanh()).collect();
            dot(readout, &t)
        }
        Teacher::TwoTap => {
            let a = dot(readout, &v);
            let b = dot(second.expect("two-tap 需要第二读出向量"), &v);
            a * b
        }
        Teacher::DeepSign => {
            let s: Vec<f32> = v.iter().map(|a| sign_f32(*a)).collect();
            dot(readout, &s)
        }
    }
}
