//! 任务权重调优：目标 / 候选网格 / 合并方式可配的坐标下降（从 `task.rs` 拆出）。

use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::task::{
    TaskVector, merge_tasks_weighted, merge_tasks_weighted_dare, merge_tasks_weighted_per_task_dare,
};
use crate::trit::Trit;

/// 用验证集选任务权重的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct WeightTuning {
    weights: Vec<f32>,
    loss: usize,
    initial_loss: usize,
    evaluations: usize,
}

impl WeightTuning {
    /// 选出的权重（与任务顺序对应）。
    #[must_use]
    pub fn weights(&self) -> &[f32] {
        &self.weights
    }

    /// 调优后的验证集失配数。
    #[must_use]
    pub fn loss(&self) -> usize {
        self.loss
    }

    /// 等权起点的验证集失配数。
    #[must_use]
    pub fn initial_loss(&self) -> usize {
        self.initial_loss
    }

    /// 实际做了多少次“合并 + 验证集前向”。
    #[must_use]
    pub fn evaluations(&self) -> usize {
        self.evaluations
    }
}

/// 坐标下降的候选权重：0 表示忽略该任务，2 表示加倍。
/// 默认调权候选网格：0（忽略）、0.5、1、2。
pub const DEFAULT_TUNE_GRID: [f32; 4] = [0.0, 0.5, 1.0, 2.0];

/// 调权目标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuneMetric {
    /// 逐样本完全匹配（默认）。
    Exact,
    /// 逐位 Hamming 失配数。
    Hamming,
    /// 三值输出分类错误数（一维按符号）。
    Argmax,
    /// f32 readout 分类错误数。
    Readout,
}

/// 调权评估时的合并方式（与最终合并路径对齐）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TuneMerge {
    /// 只做加权投票合并（不含 DARE）。
    Plain,
    /// 加权投票后做一次坐标级 DARE。
    Dare {
        /// DARE 丢弃率。
        rate: f32,
        /// DARE 散列种子。
        seed: u64,
    },
    /// 每个任务先 DARE 再投票。
    PerTaskDare {
        /// DARE 丢弃率。
        rate: f32,
        /// DARE 散列种子。
        seed: u64,
    },
}

/// 用验证集坐标下降地选任务权重：每轮对每个任务试候选集，保留验证集 `loss` 更小的。
///
/// 从等权（全 1）出发；每轮按任务顺序更新，只有 `loss` 严格下降才接受；某轮没有任何改进
/// 就提前停。评估的是**加权投票合并**（不含 DARE），调用方拿到权重后再执行自己需要的
/// 合并次序。`rounds=0` 返回等权与等权 loss（只做一次评估）。空任务判错。
/// 用验证集（逐样本完全匹配）选任务权重。
pub fn tune_task_weights(
    base: &Net,
    tasks: &[TaskVector],
    oracle: &Oracle,
    rounds: usize,
) -> Result<WeightTuning> {
    tune_task_weights_metric(
        base,
        tasks,
        oracle,
        rounds,
        TuneMetric::Exact,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
    )
}

/// 用验证集的**逐位 Hamming 失配数**选任务权重。
///
/// 多类 one-hot 输出下，逐样本完全匹配几乎恒满（无信号）；逐位统计能把“接近”与“全错”
/// 区分开。目标越小越好，其余行为与 [`tune_task_weights`] 相同。
pub fn tune_task_weights_hamming(
    base: &Net,
    tasks: &[TaskVector],
    oracle: &Oracle,
    rounds: usize,
) -> Result<WeightTuning> {
    tune_task_weights_metric(
        base,
        tasks,
        oracle,
        rounds,
        TuneMetric::Hamming,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
    )
}

/// 通用调权：`evaluate` 给一个候选网络打分，越小越好。
///
/// 从等权（全 1）出发；每轮对每个任务试候选集，只有分数严格下降才接受；某轮没有任何
/// 改进就提前停。评估的是**加权投票合并**（不含 DARE）。`rounds=0` 返回等权与等权分数
/// （只做一次评估）。空任务判错。
pub fn tune_task_weights_with(
    base: &Net,
    tasks: &[TaskVector],
    rounds: usize,
    evaluate: impl Fn(&Net) -> Result<usize>,
) -> Result<WeightTuning> {
    validate_grid(&DEFAULT_TUNE_GRID)?;
    tune_with(
        base,
        tasks,
        rounds,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
        &evaluate,
    )
}

/// 可配置目标 / 网格 / 合并方式的调权入口。
pub fn tune_task_weights_metric(
    base: &Net,
    tasks: &[TaskVector],
    oracle: &Oracle,
    rounds: usize,
    metric: TuneMetric,
    grid: &[f32],
    merge: TuneMerge,
) -> Result<WeightTuning> {
    validate_grid(grid)?;
    match metric {
        TuneMetric::Exact => tune_with(base, tasks, rounds, grid, merge, &|net| oracle.loss(net)),
        TuneMetric::Hamming => tune_with(base, tasks, rounds, grid, merge, &|net| {
            hamming_loss(oracle, net)
        }),
        TuneMetric::Argmax => tune_with(base, tasks, rounds, grid, merge, &|net| {
            argmax_loss(oracle, net)
        }),
        TuneMetric::Readout => tune_with(base, tasks, rounds, grid, merge, &|net| {
            readout_loss(oracle, net)
        }),
    }
}

fn validate_grid(grid: &[f32]) -> Result<()> {
    if grid.is_empty() {
        return Err(Error::Shape {
            what: "grid",
            expected: 1,
            got: 0,
        });
    }
    for (index, value) in grid.iter().enumerate() {
        if !value.is_finite() || *value < 0.0 {
            return Err(Error::InvalidGrid {
                index,
                value: *value,
            });
        }
    }
    Ok(())
}

fn tune_with(
    base: &Net,
    tasks: &[TaskVector],
    rounds: usize,
    grid: &[f32],
    merge: TuneMerge,
    evaluate: &dyn Fn(&Net) -> Result<usize>,
) -> Result<WeightTuning> {
    if tasks.is_empty() {
        return Err(Error::Audit("调优需要至少一个任务".into()));
    }
    let merged = |weights: &[f32]| -> Result<Net> {
        match merge {
            TuneMerge::Plain => merge_tasks_weighted(base, tasks, weights),
            TuneMerge::Dare { rate, seed } => {
                merge_tasks_weighted_dare(base, tasks, weights, rate, seed)
            }
            TuneMerge::PerTaskDare { rate, seed } => {
                merge_tasks_weighted_per_task_dare(base, tasks, weights, rate, seed)
            }
        }
    };
    let mut weights = vec![1.0_f32; tasks.len()];
    let mut best_loss = evaluate(&merged(&weights)?)?;
    let initial_loss = best_loss;
    let mut evaluations = 1usize;
    for _ in 0..rounds {
        let mut improved = false;
        for index in 0..weights.len() {
            for &candidate in grid {
                if candidate == weights[index] {
                    continue;
                }
                let mut trial = weights.clone();
                trial[index] = candidate;
                let loss = evaluate(&merged(&trial)?)?;
                evaluations += 1;
                if loss < best_loss {
                    best_loss = loss;
                    weights[index] = candidate;
                    improved = true;
                }
            }
        }
        if !improved {
            break;
        }
    }
    Ok(WeightTuning {
        weights,
        loss: best_loss,
        initial_loss,
        evaluations,
    })
}

/// 用验证集的**分类错误数**选任务权重。
///
/// 多维输出按最大 trit 的位置比较；一维输出按符号分类（`>0` 记 1，否则 0），
/// 因此二分类不会退化成“所有样本同类”。
pub fn tune_task_weights_argmax(
    base: &Net,
    tasks: &[TaskVector],
    oracle: &Oracle,
    rounds: usize,
) -> Result<WeightTuning> {
    tune_task_weights_metric(
        base,
        tasks,
        oracle,
        rounds,
        TuneMetric::Argmax,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
    )
}

/// 分类错误数：输出与期望分别分类后比较。
fn argmax_loss(oracle: &Oracle, net: &Net) -> Result<usize> {
    let actual = oracle.outputs_of(net)?;
    let expected = oracle.outputs();
    Ok(actual
        .iter()
        .zip(expected)
        .map(|(got, want)| usize::from(classify(got) != classify(want)))
        .sum())
}

/// 一维按符号分类（`>0` 为 1），多维按最大 trit 的位置。
fn classify(values: &[Trit]) -> usize {
    if values.len() == 1 {
        return usize::from(values[0].to_i8() > 0);
    }
    argmax(values)
}

fn argmax(values: &[Trit]) -> usize {
    let mut best = 0usize;
    let mut best_value = i8::MIN;
    for (index, trit) in values.iter().enumerate() {
        let value = trit.to_i8();
        if value > best_value {
            best_value = value;
            best = index;
        }
    }
    best
}

/// 用验证集的 **f32 readout** 分类错误数选任务权重。
///
/// 与 [`tune_task_weights_argmax`] 的区别：这里用 `Net::readout` 的连续分数取最大，
/// 不先把输出压成三值，能区分“都是 Pos 但分数不同”的类别。
pub fn tune_task_weights_readout(
    base: &Net,
    tasks: &[TaskVector],
    oracle: &Oracle,
    rounds: usize,
) -> Result<WeightTuning> {
    tune_task_weights_metric(
        base,
        tasks,
        oracle,
        rounds,
        TuneMetric::Readout,
        &DEFAULT_TUNE_GRID,
        TuneMerge::Plain,
    )
}

fn readout_loss(oracle: &Oracle, net: &Net) -> Result<usize> {
    let mut errors = 0usize;
    for (input, want) in oracle.inputs().iter().zip(oracle.outputs()) {
        let input_f32: Vec<f32> = input.iter().map(|trit| f32::from(trit.to_i8())).collect();
        let got = net.readout(&input_f32)?;
        let index = if got.len() == 1 {
            usize::from(got[0] > 0.0)
        } else {
            argmax_f32(&got)
        };
        if index != classify(want) {
            errors += 1;
        }
    }
    Ok(errors)
}

fn argmax_f32(values: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (index, value) in values.iter().enumerate() {
        if *value > best_value {
            best_value = *value;
            best = index;
        }
    }
    best
}

/// 逐位 Hamming 失配数（所有样本、所有输出位）。
fn hamming_loss(oracle: &Oracle, net: &Net) -> Result<usize> {
    let actual = oracle.outputs_of(net)?;
    let expected = oracle.outputs();
    Ok(actual
        .iter()
        .zip(expected)
        .map(|(got, want)| got.iter().zip(want).filter(|(a, b)| a != b).count())
        .sum())
}
