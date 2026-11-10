//! 行内定位：在承重行里找具体 trit。
//!
//! 两级读取：
//!
//! 1. [`localize`] 先用自适应行探针（[`crate::probe::probe_rows_adaptive`]）选出 top 行；
//! 2. [`localize_row`] 在单行内逐位置（权重 + 偏置）试替代值，整验证集复核。
//!
//! 单行成本 `2 × 位置数 × 样本数`。`max_trits>0` 且位置数超过预算时，按**该位置在样本上的
//! 实际活跃次数**选点（权重为零或入参恒为零的位置得 0 分），报告里标
//! `selection=adaptive` / `sampled=true` / `stride=0`，不做静默截断。
//! 打分最多看 256 条样本，仍是启发式。

use crate::analyze::edit_key;
use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::probe::{Baseline, ProbeReport, effect, probe_rows_adaptive};
use crate::trit::Trit;

/// 单个 trit / 偏置编辑的整验证集影响。
#[derive(Clone, Debug, PartialEq)]
pub struct TritImpact {
    /// 候选编辑。
    pub edit: Edit,
    /// 编辑前的失配数。
    pub before_loss: usize,
    /// 编辑后的失配数。
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

/// 行内选点方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalizeSelection {
    /// 位置数在预算内，全部探测。
    All,
    /// 超出预算，按位置活跃次数选点。
    Adaptive,
}

/// 单行定位结果。
#[derive(Clone, Debug, PartialEq)]
pub struct RowLocalization {
    /// 层号。
    pub layer: usize,
    /// 行号。
    pub row: usize,
    /// 行宽。
    pub cols: usize,
    /// 位置数（`cols` 个权重 + 1 个偏置）。
    pub positions: usize,
    /// 实际探测的位置数。
    pub probed: usize,
    /// 是否因 `max_trits` 抽样。
    pub sampled: bool,
    /// 选点方式。
    pub selection: LocalizeSelection,
    /// 抽样步长（`1` = 全量；`0` = 按活跃度选点，没有固定步长）。
    pub stride: usize,
    /// 评估过的候选编辑数。
    pub evaluated: usize,
    /// 按收益排序后的候选（最多 `top` 条；`top=0` 表示全部）。
    pub top: Vec<TritImpact>,
}

/// 两级定位报告。
#[derive(Clone, Debug, PartialEq)]
pub struct LocalizeReport {
    /// 第一级行探针报告。
    pub probe: ProbeReport,
    /// 每个被选行的行内定位。
    pub rows: Vec<RowLocalization>,
}

/// 两级定位：自适应选 top 行，再在每行内做 trit 级扫描。
pub fn localize(
    net: &Net,
    oracle: &Oracle,
    row_budget: usize,
    top_rows: usize,
    max_trits: usize,
    top_trits: usize,
) -> Result<LocalizeReport> {
    let probe = probe_rows_adaptive(net, oracle, row_budget, top_rows)?;
    let mut rows = Vec::with_capacity(probe.top.len());
    for impact in &probe.top {
        rows.push(localize_row(
            net,
            oracle,
            impact.layer,
            impact.row,
            top_trits,
            max_trits,
        )?);
    }
    Ok(LocalizeReport { probe, rows })
}

/// 在单行内逐位置（权重 + 偏置）试替代值，整验证集复核。
pub fn localize_row(
    net: &Net,
    oracle: &Oracle,
    layer: usize,
    row: usize,
    top: usize,
    max_trits: usize,
) -> Result<RowLocalization> {
    oracle.check_arch(net)?;
    let Some(target) = net.layer(layer) else {
        return Err(Error::OutOfBounds { layer, row, col: 0 });
    };
    if row >= target.rows() {
        return Err(Error::OutOfBounds { layer, row, col: 0 });
    }
    let cols = target.cols();
    let positions = cols + 1;
    let baseline = Baseline::new(net, oracle)?;
    let before_loss = baseline.loss;
    let (selected, stride, sampled, selection) = if max_trits == 0 || positions <= max_trits {
        (
            (0..positions).collect::<Vec<_>>(),
            1,
            false,
            LocalizeSelection::All,
        )
    } else {
        let scores = position_scores(net, oracle, layer, row, 256)?;
        let mut order: Vec<usize> = (0..positions).collect();
        order.sort_by(|left, right| {
            scores[*right]
                .cmp(&scores[*left])
                .then_with(|| left.cmp(right))
        });
        order.truncate(max_trits);
        order.sort_unstable();
        (order, 0, true, LocalizeSelection::Adaptive)
    };
    let probed = selected.len();
    let mut impacts = Vec::new();
    for position in selected {
        if position == cols {
            let current = target.bias_trit(row).expect("row in range");
            for to in alternatives(current) {
                impacts.push(measure(
                    net,
                    oracle,
                    &baseline,
                    before_loss,
                    Edit::SetBias { layer, row, to },
                )?);
            }
        } else {
            let current = target.trit(row, position).expect("position in range");
            for to in alternatives(current) {
                impacts.push(measure(
                    net,
                    oracle,
                    &baseline,
                    before_loss,
                    Edit::SetTrit {
                        layer,
                        row,
                        col: position,
                        to,
                    },
                )?);
            }
        }
    }
    let evaluated = impacts.len();
    impacts.sort_by(|left, right| {
        right
            .gain
            .cmp(&left.gain)
            .then(left.after_loss.cmp(&right.after_loss))
            .then(left.broken.cmp(&right.broken))
            .then(right.changed.cmp(&left.changed))
            .then_with(|| edit_key(&left.edit).cmp(&edit_key(&right.edit)))
    });
    if top > 0 {
        impacts.truncate(top);
    }
    Ok(RowLocalization {
        layer,
        row,
        cols,
        positions,
        probed,
        sampled,
        selection,
        stride,
        evaluated,
        top: impacts,
    })
}

/// 每个位置在样本上的活跃次数：权重非零且该列输入非零才计 1；偏置只看自身是否非零。
///
/// 隐藏层的“输入”是上一层激活（`sign(pre_activation)`），不是原始样本。
fn position_scores(
    net: &Net,
    oracle: &Oracle,
    layer: usize,
    row: usize,
    max_samples: usize,
) -> Result<Vec<usize>> {
    let target = net
        .layer(layer)
        .ok_or(Error::OutOfBounds { layer, row, col: 0 })?;
    let cols = target.cols();
    let mut scores = vec![0usize; cols + 1];
    if oracle.is_empty() {
        return Ok(scores);
    }
    let take = if max_samples == 0 {
        oracle.len()
    } else {
        max_samples.min(oracle.len())
    };
    let stride = oracle.len().div_ceil(take);
    let bias_live = target.bias_trit(row).expect("row in range") != Trit::Zero;
    for index in (0..oracle.len()).step_by(stride).take(take) {
        let input = &oracle.inputs()[index];
        let activation: Vec<Trit> = if layer == 0 {
            input.clone()
        } else {
            net.pre_activations(input)?[layer - 1]
                .iter()
                .map(|value| Trit::sign_of(*value))
                .collect()
        };
        for (col, active) in activation.iter().enumerate().take(cols) {
            if *active != Trit::Zero
                && target.trit(row, col).expect("position in range") != Trit::Zero
            {
                scores[col] += 1;
            }
        }
        if bias_live {
            scores[cols] += 1;
        }
    }
    Ok(scores)
}

fn alternatives(current: Trit) -> Vec<Trit> {
    Trit::ALL
        .into_iter()
        .filter(|candidate| *candidate != current)
        .collect()
}

fn measure(
    net: &Net,
    oracle: &Oracle,
    baseline: &Baseline,
    before_loss: usize,
    edit: Edit,
) -> Result<TritImpact> {
    let patch = Patch::new(vec![edit.clone()]);
    let effect = effect(net, oracle, &patch, baseline)?;
    Ok(TritImpact {
        edit,
        before_loss,
        after_loss: effect.after_loss,
        gain: effect.gain,
        changed: effect.changed,
        fixed: effect.fixed,
        broken: effect.broken,
    })
}
