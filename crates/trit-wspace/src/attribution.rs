//! 单参数能力归属：改一个 trit 会失去哪些样本/能力，是否唯一支撑某个能力。
//!
//! 对每个参数做一次重启式编辑（翻转或置零），重算整个验证集，记录：
//!
//! - `lost`：改前对、改后错的样本；
//! - `gained`：改前错、改后对的样本；
//! - `changed`：输出发生变化的样本（含错→错）；
//! - `specificity = lost / changed`：越高越「专」，`1.0` 表示它只影响它该影响的样本。
//!
//! 相同的 `lost` 集合归为一种「能力」；若某能力只由一个参数支撑，该参数被标为 `sole`。
//! 成本是 `参数数 × 样本数` 次前向，用 `max_params` / `--layer` / `--row` 限流。

use std::collections::BTreeMap;

use crate::edit::{Edit, Patch};
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 编辑方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditMode {
    /// 非零取反，零变 `Pos`。
    Flip,
    /// 直接置零。
    Zero,
}

impl EditMode {
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

/// 参数坐标（`col = None` 表示该行的偏置）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ParamRef {
    /// 层号。
    pub layer: usize,
    /// 行号。
    pub row: usize,
    /// 列号；`None` 表示偏置。
    pub col: Option<usize>,
}

/// 单个参数的归属。
#[derive(Clone, Debug, PartialEq)]
pub struct Attribution {
    /// 参数坐标。
    pub param: ParamRef,
    /// 原值。
    pub from: Trit,
    /// 编辑后的值。
    pub to: Trit,
    /// 失去的样本（改前对、改后错）。
    pub lost: Vec<usize>,
    /// 得到的样本（改前错、改后对）。
    pub gained: Vec<usize>,
    /// 输出变化的样本（含错→错）。
    pub changed: Vec<usize>,
    /// `lost / changed`；`1.0` 表示只影响该影响的样本。
    pub specificity: f64,
    /// 该参数唯一支撑的能力索引。
    pub sole: Vec<usize>,
}

/// 一种能力：由「改哪些参数会失去这些样本」定义。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capability {
    /// 该能力对应的样本集合。
    pub lost: Vec<usize>,
    /// 改这些参数会失去该能力。
    pub support: Vec<ParamRef>,
}

/// 归属报告。
#[derive(Clone, Debug, PartialEq)]
pub struct AttributeReport {
    /// 样本数。
    pub samples: usize,
    /// 实际扫描的参数数。
    pub params: usize,
    /// 基线失配数。
    pub baseline_loss: usize,
    /// 按 `lost.len()` 降序的 top 参数。
    pub top: Vec<Attribution>,
    /// 按 `lost` 集合分组的能力。
    pub capabilities: Vec<Capability>,
    /// 专才参数：`specificity == 1.0` 且 `lost` 不超过 4 个样本。
    pub specialists: Vec<ParamRef>,
}

/// 归属选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttributeOptions {
    /// 编辑方式。
    pub mode: EditMode,
    /// 只扫这一层。
    pub layer: Option<usize>,
    /// 只扫这一行（需与 `layer` 同时给）。
    pub row: Option<usize>,
    /// 最多扫描多少个参数（0 = 全部）。
    pub max_params: usize,
    /// 报告保留多少条。
    pub top: usize,
}

impl Default for AttributeOptions {
    fn default() -> Self {
        Self {
            mode: EditMode::Flip,
            layer: None,
            row: None,
            max_params: 4096,
            top: 16,
        }
    }
}

fn params(net: &Net, options: &AttributeOptions) -> Vec<ParamRef> {
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

/// 逐参数算能力归属。
pub fn attribute(
    net: &Net,
    oracle: &Oracle,
    options: &AttributeOptions,
) -> Result<AttributeReport> {
    oracle.check_arch(net)?;
    let want = oracle.outputs();
    let baseline = oracle.outputs_of(net)?;
    let all_params = params(net, options);
    let mut all = Vec::with_capacity(all_params.len());
    for param in &all_params {
        let layer = &net.layers()[param.layer];
        let from = match param.col {
            Some(col) => layer.trit(param.row, col).expect("trit"),
            None => layer.bias_trit(param.row).expect("bias"),
        };
        let to = options.mode.target(from);
        if to == from {
            continue;
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
        let mut edited = net.clone();
        Patch::new(vec![edit]).apply(&mut edited)?;
        let after = oracle.outputs_of(&edited)?;
        let mut lost = Vec::new();
        let mut gained = Vec::new();
        let mut changed = Vec::new();
        for index in 0..want.len() {
            if baseline[index] != after[index] {
                changed.push(index);
            }
            if baseline[index] == want[index] && after[index] != want[index] {
                lost.push(index);
            }
            if baseline[index] != want[index] && after[index] == want[index] {
                gained.push(index);
            }
        }
        let specificity = if changed.is_empty() {
            0.0
        } else {
            lost.len() as f64 / changed.len() as f64
        };
        all.push(Attribution {
            param: *param,
            from,
            to,
            lost,
            gained,
            changed,
            specificity,
            sole: Vec::new(),
        });
    }
    let mut groups: BTreeMap<Vec<usize>, Vec<ParamRef>> = BTreeMap::new();
    for attribution in &all {
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
    for attribution in &mut all {
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
    let mut specialists: Vec<ParamRef> = all
        .iter()
        .filter(|a| a.specificity == 1.0 && !a.lost.is_empty() && a.lost.len() <= 4)
        .map(|a| a.param)
        .collect();
    specialists.sort();
    specialists.dedup();
    let mut top = all;
    top.sort_by_key(|a| std::cmp::Reverse(a.lost.len()));
    if options.top > 0 {
        top.truncate(options.top);
    }
    Ok(AttributeReport {
        samples: want.len(),
        params: all_params.len(),
        baseline_loss: oracle.loss(net)?,
        top,
        capabilities,
        specialists,
    })
}
