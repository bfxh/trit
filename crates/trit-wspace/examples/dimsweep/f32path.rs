//! dimsweep 的 f32 对照路径：降维但**不三值化**，用来分离投影损失与量化损失。
//!
//! 从 `judge.rs` 拆出：module_budget 台账只收紧、不抬高单文件行数。

use crate::basis::{Sample, dot, project, sign_of};
use crate::judge::{Teacher, sign_f32};
use trit_wspace::safetensors::Weights;

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
                Teacher::DeepSign => {
                    let s: Vec<f32> = v.iter().map(|a| sign_f32(*a)).collect();
                    dot(readout, &s)
                }
            };
            sign_of(score) == s.teacher
        })
        .count();
    hit as f32 / samples.len() as f32
}
