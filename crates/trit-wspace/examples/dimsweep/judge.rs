//! dimsweep 的判据：线性 / tanh / two-tap 三种教师，以及降维 f32 对照。
//!
//! 从 `basis.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::{Sample, dot, project, sign_of, teacher_forward};
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
}

pub fn parse_teacher(name: &str) -> Teacher {
    match name {
        "linear" => Teacher::Linear,
        "tanh" => Teacher::Tanh,
        "two-tap" => Teacher::TwoTap,
        other => panic!("未知 --teacher {other}（linear | tanh | two-tap）"),
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
    }
}

/// 降维 **f32** 网络（权重与输入都不三值化）在给定教师下的准确率。
///
/// 三值路径的对照：两者之差就是「三值化 + sign 判决」的代价。对非线性教师也成立，
/// 所以不能再用 `w_eff` 的线性公式。
pub fn f32_reduced_accuracy(
    samples: &[Sample],
    comps: &[Vec<f64>],
    query: &Weights,
    readout: &[f32],
    second: Option<&[f32]>,
    dim: usize,
    kind: Teacher,
) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let k = comps.len();
    let mut wk = vec![0.0f64; query.rows * k];
    for row in 0..query.rows {
        let w = &query.values[row * dim..(row + 1) * dim];
        for (i, u) in comps.iter().enumerate() {
            wk[row * k + i] = u.iter().zip(w).map(|(a, b)| a * f64::from(*b)).sum();
        }
    }
    let hit = samples
        .iter()
        .filter(|s| {
            let xk = project(&s.x, comps);
            let v: Vec<f32> = (0..query.rows)
                .map(|row| {
                    let w = &wk[row * k..(row + 1) * k];
                    w.iter()
                        .zip(&xk)
                        .map(|(a, b)| a * f64::from(*b))
                        .sum::<f64>() as f32
                })
                .collect();
            let score = match kind {
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
            };
            sign_of(score) == s.teacher
        })
        .count();
    hit as f32 / samples.len() as f32
}
