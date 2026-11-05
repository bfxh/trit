//! 行级消融探针：大网读取。
//!
//! 逐行清零 / 取反，测行为漂移。成本是 `2 × 探针行数 × 样本数`，与**行宽无关**
//! （每行只做一次前向，不管这行有多少权重），所以比逐权重分析更适合大网。
//!
//! `budget>0` 且总行数超过预算时，按固定步长均匀抽样，并在报告里标
//! `sampled=true` / `stride`——这是**明确标注的抽样**，不是静默截断。

use std::collections::HashSet;

use crate::edit::{Edit, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

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
    /// 是否因预算做了均匀抽样。
    pub sampled: bool,
    /// 抽样步长（`1` = 全量）。
    pub stride: usize,
    /// 按行为漂移排序后的行（最多 `top` 条；`top=0` 表示全部）。
    pub top: Vec<RowImpact>,
}

/// 逐行清零 / 取反，返回按行为漂移排序的影响。
pub fn probe_rows(net: &Net, oracle: &Oracle, budget: usize, top: usize) -> Result<ProbeReport> {
    oracle.check_arch(net)?;
    let baseline_outputs = oracle.outputs_of(net)?;
    let baseline_mismatches: Vec<usize> = baseline_outputs
        .iter()
        .zip(oracle.outputs())
        .enumerate()
        .filter_map(|(index, (got, want))| (got != want).then_some(index))
        .collect();
    let baseline_set: HashSet<usize> = baseline_mismatches.iter().copied().collect();
    let before_loss = baseline_mismatches.len();

    let mut all_rows = Vec::new();
    for (layer, weights) in net.layers().iter().enumerate() {
        for row in 0..weights.rows() {
            all_rows.push((layer, row));
        }
    }
    let rows = all_rows.len();
    let (selected, stride, sampled) = if budget == 0 || rows <= budget {
        (all_rows, 1, false)
    } else {
        let stride = rows.div_ceil(budget);
        let selected = all_rows.iter().step_by(stride).copied().collect();
        (selected, stride, true)
    };
    let probed_rows = selected.len();

    let mut impacts = Vec::with_capacity(probed_rows);
    let baseline = Baseline {
        outputs: &baseline_outputs,
        mismatches: &baseline_mismatches,
        set: &baseline_set,
        loss: before_loss,
    };
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
        stride,
        top: impacts,
    })
}

struct Baseline<'a> {
    outputs: &'a [Vec<Trit>],
    mismatches: &'a [usize],
    set: &'a HashSet<usize>,
    loss: usize,
}

fn effect(net: &Net, oracle: &Oracle, patch: &Patch, baseline: &Baseline<'_>) -> Result<RowEffect> {
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
