//! 两阶段参数扫描：先用小样本粗筛，再对候选做全量复核。
//!
//! `attribute` 对每个参数都做一次全验证集前向，成本是「参数数 × 样本数」。
//! `scan` 先用 `sample_budget` 个样本粗筛，只对「在子集上有影响」的参数做全量复核，
//! 用 `ScanStats` 明确标出剪枝比例；`sample_budget = 0` 时退化为全量扫描。

use std::collections::BTreeMap;

use crate::attribution::{AttributeReport, Attribution, Capability, EditMode, ParamRef};
use crate::edit::{Edit, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 扫描选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    /// 编辑方式。
    pub mode: EditMode,
    /// 只扫这一层。
    pub layer: Option<usize>,
    /// 只扫这一行。
    pub row: Option<usize>,
    /// 最多扫描多少个参数（0 = 全部）。
    pub max_params: usize,
    /// 报告保留多少条。
    pub top: usize,
    /// 粗筛样本数（0 = 全量，不做两阶段）。
    pub sample_budget: usize,
    /// 粗筛后最多复核多少个参数（0 = 不额外限制）。
    pub refine: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            mode: EditMode::Flip,
            layer: None,
            row: None,
            max_params: 4096,
            top: 16,
            sample_budget: 0,
            refine: 0,
        }
    }
}

/// 扫描统计。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanStats {
    /// 作用域内的参数总数。
    pub total_params: usize,
    /// 粗筛后保留的参数数（全量时等于 `total_params`）。
    pub screened: usize,
    /// 实际全量复核的参数数。
    pub refined: usize,
    /// 是否用了两阶段。
    pub sampled: bool,
}

/// 扫描结果。
#[derive(Clone, Debug, PartialEq)]
pub struct ScanResult {
    /// 归属报告（与 `attribute` 同型）。
    pub report: AttributeReport,
    /// 剪枝统计。
    pub stats: ScanStats,
}

fn target(mode: EditMode, from: Trit) -> Trit {
    match mode {
        EditMode::Zero => Trit::Zero,
        EditMode::Flip => match from {
            Trit::Pos => Trit::Neg,
            Trit::Neg => Trit::Pos,
            Trit::Zero => Trit::Pos,
        },
    }
}

fn edit_for(net: &Net, param: &ParamRef, mode: EditMode) -> Option<(Edit, Trit, Trit)> {
    let layer = &net.layers()[param.layer];
    let from = match param.col {
        Some(col) => layer.trit(param.row, col)?,
        None => layer.bias_trit(param.row)?,
    };
    let to = target(mode, from);
    if to == from {
        return None;
    }
    let edit = match param.col {
        Some(col) => Edit::SetTrit {
            layer: param.layer,
            row: param.row,
            col,
            to,
        },
        None => Edit::SetBias {
            layer: param.layer,
            row: param.row,
            to,
        },
    };
    Some((edit, from, to))
}

fn all_params(net: &Net, options: &ScanOptions) -> Vec<ParamRef> {
    let mut out = Vec::new();
    for (index, layer) in net.layers().iter().enumerate() {
        if options.layer.is_some_and(|wanted| wanted != index) {
            continue;
        }
        for row in 0..layer.rows() {
            if options.row.is_some_and(|wanted| wanted != row) {
                continue;
            }
            for col in 0..layer.cols() {
                out.push(ParamRef {
                    layer: index,
                    row,
                    col: Some(col),
                });
            }
            out.push(ParamRef {
                layer: index,
                row,
                col: None,
            });
        }
    }
    if options.max_params > 0 {
        out.truncate(options.max_params);
    }
    out
}

fn diff(
    before: &[Vec<Trit>],
    after: &[Vec<Trit>],
    want: &[Vec<Trit>],
) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut lost = Vec::new();
    let mut gained = Vec::new();
    let mut changed = Vec::new();
    for index in 0..want.len() {
        if before[index] != after[index] {
            changed.push(index);
        }
        if before[index] == want[index] && after[index] != want[index] {
            lost.push(index);
        }
        if before[index] != want[index] && after[index] == want[index] {
            gained.push(index);
        }
    }
    (lost, gained, changed)
}

fn evaluate(
    net: &Net,
    oracle: &Oracle,
    before: &[Vec<Trit>],
    param: &ParamRef,
    mode: EditMode,
) -> Result<Option<Attribution>> {
    let Some((edit, from, to)) = edit_for(net, param, mode) else {
        return Ok(None);
    };
    let mut trial = net.clone();
    Patch::new(vec![edit]).apply(&mut trial)?;
    let after = oracle.outputs_of(&trial)?;
    let want = oracle.outputs();
    let (lost, gained, changed) = diff(before, &after, want);
    let specificity = if changed.is_empty() {
        0.0
    } else {
        lost.len() as f64 / changed.len() as f64
    };
    Ok(Some(Attribution {
        param: *param,
        from,
        to,
        lost,
        gained,
        changed,
        specificity,
        sole: Vec::new(),
    }))
}

/// 两阶段扫描。
pub fn scan(net: &Net, oracle: &Oracle, options: &ScanOptions) -> Result<ScanResult> {
    oracle.check_arch(net)?;
    let all = all_params(net, options);
    let total_params = all.len();
    let sampled = options.sample_budget > 0 && options.sample_budget < oracle.len();
    let mut params = all;
    if sampled {
        let budget = options.sample_budget.min(oracle.len());
        let before = oracle.outputs_of(net)?;
        let want = oracle.outputs();
        let mut scored = Vec::new();
        for param in &params {
            let Some((edit, _, _)) = edit_for(net, param, options.mode) else {
                continue;
            };
            let mut trial = net.clone();
            Patch::new(vec![edit]).apply(&mut trial)?;
            let after = oracle.outputs_of(&trial)?;
            let mut hits = 0usize;
            for index in 0..budget {
                if before[index] != after[index] {
                    hits += 1;
                }
                if before[index] == want[index] && after[index] != want[index] {
                    hits += 1;
                }
            }
            if hits > 0 {
                scored.push((*param, hits));
            }
        }
        scored.sort_by_key(|(_, hits)| std::cmp::Reverse(*hits));
        if options.refine > 0 {
            scored.truncate(options.refine);
        }
        params = scored.into_iter().map(|(param, _)| param).collect();
    }
    let screened = params.len();
    let before = oracle.outputs_of(net)?;
    let mut evaluated = Vec::with_capacity(params.len());
    for param in &params {
        if let Some(attribution) = evaluate(net, oracle, &before, param, options.mode)? {
            evaluated.push(attribution);
        }
    }
    let mut groups: BTreeMap<Vec<usize>, Vec<ParamRef>> = BTreeMap::new();
    for attribution in &evaluated {
        if !attribution.lost.is_empty() {
            groups
                .entry(attribution.lost.clone())
                .or_default()
                .push(attribution.param);
        }
    }
    let capabilities: Vec<Capability> = groups
        .iter()
        .map(|(lost, support)| Capability {
            lost: lost.clone(),
            support: support.clone(),
        })
        .collect();
    for attribution in &mut evaluated {
        if attribution.lost.is_empty() {
            continue;
        }
        if let Some(index) = capabilities
            .iter()
            .position(|cap| cap.lost == attribution.lost)
            && capabilities[index].support.len() == 1
        {
            attribution.sole.push(index);
        }
    }
    let mut specialists: Vec<ParamRef> = evaluated
        .iter()
        .filter(|a| a.specificity == 1.0 && !a.lost.is_empty() && a.lost.len() <= 4)
        .map(|a| a.param)
        .collect();
    specialists.sort();
    specialists.dedup();
    let mut top = evaluated;
    top.sort_by_key(|a| std::cmp::Reverse(a.lost.len()));
    if options.top > 0 {
        top.truncate(options.top);
    }
    let report = AttributeReport {
        samples: oracle.len(),
        params: params.len(),
        baseline_loss: oracle.loss(net)?,
        top,
        capabilities,
        specialists,
    };
    Ok(ScanResult {
        report,
        stats: ScanStats {
            total_params,
            screened,
            refined: params.len(),
            sampled,
        },
    })
}
