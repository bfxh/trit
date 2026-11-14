//! 样本突变与输入敏感度：逐样本、逐输入位翻转/置零，测输出变化。
//!
//! 三值网络不可微；这里用**逐位翻转的有限差分**代替梯度：
//! - `per_position`：每个输入位一改会翻转多少样本——承重输入位；
//! - `per_sample`：每个样本有多脆弱——改一位就翻转的样本；
//! - `lost`：改前对、改后错；`gained`：改前错、改后对。

use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 突变方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PerturbMode {
    /// 非零取反，零变 `Pos`。
    Flip,
    /// 直接置零。
    Zero,
}

impl PerturbMode {
    fn target(self, from: Trit) -> Trit {
        match self {
            Self::Zero => Trit::Zero,
            Self::Flip => match from {
                Trit::Pos => Trit::Neg,
                Trit::Neg => Trit::Pos,
                Trit::Zero => Trit::Pos,
            },
        }
    }
}

/// 一个输入位的敏感度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PositionSensitivity {
    /// 输入位下标。
    pub position: usize,
    /// 输出翻转次数。
    pub flips: usize,
    /// 由对变错次数。
    pub lost: usize,
}

/// 一个样本的敏感度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SampleSensitivity {
    /// 样本下标。
    pub sample: usize,
    /// 输出翻转次数。
    pub flips: usize,
    /// 由对变错次数。
    pub lost: usize,
}

/// 突变报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PerturbReport {
    /// 样本数。
    pub samples: usize,
    /// 输入维度。
    pub width: usize,
    /// 基线失配数。
    pub baseline_loss: usize,
    /// 尝试的突变总数。
    pub perturbations: usize,
    /// 输出翻转总数。
    pub flipped: usize,
    /// 由对变错总数。
    pub lost: usize,
    /// 由错变对总数。
    pub gained: usize,
    /// 逐输入位敏感度。
    pub per_position: Vec<PositionSensitivity>,
    /// 逐样本敏感度。
    pub per_sample: Vec<SampleSensitivity>,
}

/// 对每个样本的每个输入位做一次突变。
pub fn perturb(net: &Net, oracle: &Oracle, mode: PerturbMode) -> Result<PerturbReport> {
    oracle.check_arch(net)?;
    let inputs = oracle.inputs();
    let want = oracle.outputs();
    let before = oracle.outputs_of(net)?;
    let width = inputs.first().map_or(0, Vec::len);
    let baseline_loss = before
        .iter()
        .zip(want)
        .filter(|(got, expected)| got != expected)
        .count();
    let mut per_position: Vec<PositionSensitivity> = (0..width)
        .map(|position| PositionSensitivity {
            position,
            flips: 0,
            lost: 0,
        })
        .collect();
    let mut per_sample = Vec::with_capacity(inputs.len());
    let mut perturbations = 0usize;
    let mut flipped = 0usize;
    let mut lost = 0usize;
    let mut gained = 0usize;
    for (sample, input) in inputs.iter().enumerate() {
        let mut sample_flips = 0usize;
        let mut sample_lost = 0usize;
        for position in 0..width {
            let mut mutated = input.clone();
            let from = mutated[position];
            let to = mode.target(from);
            if to == from {
                continue;
            }
            mutated[position] = to;
            let out = net.forward(&mutated)?;
            perturbations += 1;
            let changed = out != before[sample];
            if changed {
                flipped += 1;
                sample_flips += 1;
                per_position[position].flips += 1;
            }
            let was_right = before[sample] == want[sample];
            let now_right = out == want[sample];
            if was_right && !now_right {
                lost += 1;
                sample_lost += 1;
                per_position[position].lost += 1;
            }
            if !was_right && now_right {
                gained += 1;
            }
        }
        per_sample.push(SampleSensitivity {
            sample,
            flips: sample_flips,
            lost: sample_lost,
        });
    }
    Ok(PerturbReport {
        samples: inputs.len(),
        width,
        baseline_loss,
        perturbations,
        flipped,
        lost,
        gained,
        per_position,
        per_sample,
    })
}
