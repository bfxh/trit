//! 行级消融探针：大网读取。
//!
//! 逐行清零 / 取反，测行为漂移。成本是 `2 × 探针行数 × 样本数`，与**行宽无关**
//! （每行只做一次前向，不管这行有多少权重），所以比逐权重分析更适合大网。
//!
//! 预算不足时有两种选行方式：
//!
//! - `probe_rows`：固定步长均匀抽样（`selection = uniform`）；
//! - `probe_rows_adaptive`：先用一次前向统计每行 pre-activation 的活跃度/裕量，
//!   优先探“真正在工作”的行，并钉住输出层行（`selection = adaptive`）。
//!
//! 两种都**明确标注** `sampled` / `stride` / `selection`，不做静默截断。

use std::cmp::Ordering;
use std::collections::HashSet;

use crate::edit::{Edit, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 选行策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeSelection {
    /// 全量（预算够）。
    All,
    /// 固定步长均匀抽样。
    Uniform,
    /// 按激活活跃度/裕量自适应选行。
    Adaptive,
}

impl ProbeSelection {
    /// 线格式里的字符串。
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Uniform => "uniform",
            Self::Adaptive => "adaptive",
        }
    }
}

/// 一种行操作的影响。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowEffect {
    /// 操作后的整验证集失配数。
    pub after_loss: usize,
    /// `before_loss - after_loss`。
    pub gain: isize,
    /// 输出发生变化的样本数。
    pub changed: usize,
    /// 由错变对的样本数。
    pub fixed: usize,
    /// 由对变错的样本数。
    pub broken: usize,
}

/// 一行的消融结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowImpact {
    /// 层号。
    pub layer: usize,
    /// 行号。
    pub row: usize,
    /// 该行参数数（权重 + 偏置）。
    pub trits: usize,
    /// 清零该行的影响。
    pub zero: RowEffect,
    /// 取反该行的影响。
    pub negate: RowEffect,
}

/// 一行的静态活跃度（自适应选行用）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowScore {
    /// 层号。
    pub layer: usize,
    /// 行号。
    pub row: usize,
    /// 抽样样本里 pre-activation 非零的次数。
    pub active: usize,
    /// 抽样样本里 pre-activation 绝对值之和。
    pub margin: i64,
    /// 该行非零参数数（权重 + 偏置）。
    pub nonzero: usize,
}

/// 探针报告。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeReport {
    /// 架构。
    pub arch: Vec<usize>,
    /// 探针前的失配数。
    pub before_loss: usize,
    /// 样本数。
    pub samples: usize,
    /// 总行数。
    pub rows: usize,
    /// 实际探测的行数。
    pub probed_rows: usize,
    /// 是否因预算做了抽样。
    pub sampled: bool,
    /// 选行策略。
    pub selection: ProbeSelection,
    /// 均匀抽样的步长（`1` = 全量；自适应抽样为 `0`）。
    pub stride: usize,
    /// 按行为漂移排序后的行（最多 `top` 条；`top=0` 表示全部）。
    pub top: Vec<RowImpact>,
}

/// 逐行清零 / 取反；预算不足时按**固定步长**均匀抽样。
pub fn probe_rows(net: &Net, oracle: &Oracle, budget: usize, top: usize) -> Result<ProbeReport> {
    probe_with(net, oracle, false, budget, top)
}

/// 逐行清零 / 取反；预算不足时按**激活活跃度/裕量**自适应选行，并钉住输出层行。
pub fn probe_rows_adaptive(
    net: &Net,
    oracle: &Oracle,
    budget: usize,
    top: usize,
) -> Result<ProbeReport> {
    probe_with(net, oracle, true, budget, top)
}

fn probe_with(
    net: &Net,
    oracle: &Oracle,
    adaptive: bool,
    budget: usize,
    top: usize,
) -> Result<ProbeReport> {
    oracle.check_arch(net)?;
    let baseline = Baseline::new(net, oracle)?;
    let before_loss = baseline.loss;

    let mut all_rows = Vec::new();
    for (layer, weights) in net.layers().iter().enumerate() {
        for row in 0..weights.rows() {
            all_rows.push((layer, row));
        }
    }
    let rows = all_rows.len();
    let (selected, stride, sampled, selection) = if budget == 0 || rows <= budget {
        (all_rows, 1, false, ProbeSelection::All)
    } else if adaptive {
        (
            select_adaptive(net, oracle, budget)?,
            0,
            true,
            ProbeSelection::Adaptive,
        )
    } else {
        let stride = rows.div_ceil(budget);
        let selected = all_rows.iter().step_by(stride).copied().collect();
        (selected, stride, true, ProbeSelection::Uniform)
    };
    let probed_rows = selected.len();

    let mut impacts = Vec::with_capacity(probed_rows);
    for (layer, row) in selected {
        let cols = net.layer(layer).expect("row came from net").cols();
        let mut zero_edits = Vec::with_capacity(cols + 1);
        for col in 0..cols {
            zero_edits.push(Edit::SetTrit {
                layer,
                row,
                col,
                to: Trit::Zero,
            });
        }
        zero_edits.push(Edit::SetBias {
            layer,
            row,
            to: Trit::Zero,
        });
        let zero_patch = Patch::new(zero_edits);
        let negate_patch = Patch::new(vec![Edit::NegateRow { layer, row }]);
        let zero = effect(net, oracle, &zero_patch, &baseline)?;
        let negate = effect(net, oracle, &negate_patch, &baseline)?;
        impacts.push(RowImpact {
            layer,
            row,
            trits: cols + 1,
            zero,
            negate,
        });
    }
    impacts.sort_by(|left, right| {
        let left_changed = left.zero.changed + left.negate.changed;
        let right_changed = right.zero.changed + right.negate.changed;
        right_changed
            .cmp(&left_changed)
            .then_with(|| {
                (right.zero.gain.abs() + right.negate.gain.abs())
                    .cmp(&(left.zero.gain.abs() + left.negate.gain.abs()))
            })
            .then_with(|| left.layer.cmp(&right.layer))
            .then_with(|| left.row.cmp(&right.row))
    });
    if top > 0 {
        impacts.truncate(top);
    }
    Ok(ProbeReport {
        arch: net.arch(),
        before_loss,
        samples: oracle.len(),
        rows,
        probed_rows,
        sampled,
        selection,
        stride,
        top: impacts,
    })
}

/// 输出层行优先（最多占满预算），其余预算按活跃度/裕量从高到低。
fn select_adaptive(net: &Net, oracle: &Oracle, budget: usize) -> Result<Vec<(usize, usize)>> {
    let scores = activity_scores(net, oracle, 256)?;
    let depth = net.depth();
    let mut output: Vec<RowScore> = scores
        .iter()
        .filter(|score| score.layer + 1 == depth)
        .cloned()
        .collect();
    let mut hidden: Vec<RowScore> = scores
        .iter()
        .filter(|score| score.layer + 1 != depth)
        .cloned()
        .collect();
    output.sort_by(rank_score);
    hidden.sort_by(rank_score);
    let mut selected = Vec::with_capacity(budget);
    for score in output.into_iter().chain(hidden) {
        if selected.len() >= budget {
            break;
        }
        selected.push((score.layer, score.row));
    }
    Ok(selected)
}

fn rank_score(left: &RowScore, right: &RowScore) -> Ordering {
    right
        .active
        .cmp(&left.active)
        .then(right.margin.cmp(&left.margin))
        .then(right.nonzero.cmp(&left.nonzero))
        .then(left.layer.cmp(&right.layer))
        .then(left.row.cmp(&right.row))
}

/// 一次前向统计每行 pre-activation 的活跃度/裕量；样本最多取 `max_samples` 条（均匀步长）。
fn activity_scores(net: &Net, oracle: &Oracle, max_samples: usize) -> Result<Vec<RowScore>> {
    let mut scores: Vec<Vec<RowScore>> = net
        .layers()
        .iter()
        .enumerate()
        .map(|(layer, weights)| {
            (0..weights.rows())
                .map(|row| {
                    let nonzero = weights
                        .row(row)
                        .expect("row")
                        .iter()
                        .filter(|trit| **trit != Trit::Zero)
                        .count()
                        + usize::from(weights.bias_trit(row).expect("bias") != Trit::Zero);
                    RowScore {
                        layer,
                        row,
                        active: 0,
                        margin: 0,
                        nonzero,
                    }
                })
                .collect()
        })
        .collect();
    let take = if max_samples == 0 {
        oracle.len()
    } else {
        max_samples.min(oracle.len())
    };
    let stride = oracle.len().div_ceil(take);
    for index in (0..oracle.len()).step_by(stride).take(take) {
        let pre = net.pre_activations(&oracle.inputs()[index])?;
        for (layer, rows) in pre.iter().enumerate() {
            for (row, &value) in rows.iter().enumerate() {
                if value != 0 {
                    scores[layer][row].active += 1;
                    scores[layer][row].margin += i64::from(value.abs());
                }
            }
        }
    }
    Ok(scores.into_iter().flatten().collect())
}

pub(crate) struct Baseline {
    outputs: Vec<Vec<Trit>>,
    mismatches: Vec<usize>,
    set: HashSet<usize>,
    pub(crate) loss: usize,
}

impl Baseline {
    pub(crate) fn new(net: &Net, oracle: &Oracle) -> Result<Self> {
        let outputs = oracle.outputs_of(net)?;
        let mismatches: Vec<usize> = outputs
            .iter()
            .zip(oracle.outputs())
            .enumerate()
            .filter_map(|(index, (got, want))| (got != want).then_some(index))
            .collect();
        let set = mismatches.iter().copied().collect();
        Ok(Self {
            loss: mismatches.len(),
            outputs,
            mismatches,
            set,
        })
    }
}

pub(crate) fn effect(
    net: &Net,
    oracle: &Oracle,
    patch: &Patch,
    baseline: &Baseline,
) -> Result<RowEffect> {
    let mut trial = net.clone();
    patch.apply(&mut trial)?;
    let outputs = oracle.outputs_of(&trial)?;
    let mismatches: Vec<usize> = outputs
        .iter()
        .zip(oracle.outputs())
        .enumerate()
        .filter_map(|(index, (got, want))| (got != want).then_some(index))
        .collect();
    let after_set: HashSet<usize> = mismatches.iter().copied().collect();
    let changed = baseline
        .outputs
        .iter()
        .zip(&outputs)
        .filter(|(left, right)| left != right)
        .count();
    let fixed = baseline
        .mismatches
        .iter()
        .filter(|index| !after_set.contains(index))
        .count();
    let broken = mismatches
        .iter()
        .filter(|index| !baseline.set.contains(index))
        .count();
    Ok(RowEffect {
        after_loss: mismatches.len(),
        gain: baseline.loss as isize - mismatches.len() as isize,
        changed,
        fixed,
        broken,
    })
}
