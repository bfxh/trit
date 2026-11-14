//! 多参数/结构化改动的收益预测。
//!
//! 给一组编辑（用户 patch，或整行/整层自动生成），一次算出：
//!
//! - 联合 `before/after loss` 与 `lost/gained/changed` 样本；
//! - 每个编辑单独施加的 `gain`；
//! - `synergy = 联合 gain - Σ 单点 gain`：正数表示 1+1>2，负数表示互相抵消。
//!
//! 与 `attribute` 的单参数视角互补：那边看「改一个失去什么」，这边看「一起改得到什么」。

use crate::edit::{Edit, Patch};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::trit::Trit;

/// 组编辑方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupMode {
    /// 非零取反，零变 `Pos`。
    Flip,
    /// 直接置零。
    Zero,
}

impl GroupMode {
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

/// 组编辑的粒度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupKind {
    /// 一整行（神经元）。
    Row,
    /// 一整层。
    Layer,
}

/// 单条编辑的实测影响。
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    /// 编辑。
    pub edit: Edit,
    /// 单独施加的增益。
    pub gain: isize,
    /// 单独施加时失去的样本。
    pub lost: Vec<usize>,
    /// 单独施加时得到的样本。
    pub gained: Vec<usize>,
    /// 单独施加时输出变化的样本。
    pub changed: Vec<usize>,
}

/// 一组编辑的联合影响。
#[derive(Clone, Debug, PartialEq)]
pub struct ImpactReport {
    /// 编辑前的失配数。
    pub before_loss: usize,
    /// 联合编辑后的失配数。
    pub after_loss: usize,
    /// `before_loss - after_loss`。
    pub gain: isize,
    /// 联合编辑失去的样本。
    pub lost: Vec<usize>,
    /// 联合编辑得到的样本。
    pub gained: Vec<usize>,
    /// 联合编辑后输出变化的样本。
    pub changed: Vec<usize>,
    /// 每条编辑单独施加的影响。
    pub individual: Vec<Item>,
    /// `gain - Σ individual.gain`。
    pub synergy: isize,
}

/// 生成整行/整层的编辑集。
pub fn group_edits(
    net: &Net,
    kind: GroupKind,
    layer: usize,
    row: Option<usize>,
    mode: GroupMode,
) -> Result<Vec<Edit>> {
    let Some(target_layer) = net.layers().get(layer) else {
        return Err(Error::Wire(format!("层 {layer} 越界")));
    };
    let rows: Vec<usize> = match kind {
        GroupKind::Row => vec![row.ok_or_else(|| Error::Wire("--group row 需要 --row".into()))?],
        GroupKind::Layer => (0..target_layer.rows()).collect(),
    };
    let mut edits = Vec::new();
    for row in rows {
        if row >= target_layer.rows() {
            return Err(Error::Wire(format!("行 {row} 越界")));
        }
        for col in 0..target_layer.cols() {
            if let Some(from) = target_layer.trit(row, col) {
                let to = mode.target(from);
                if to != from {
                    edits.push(Edit::SetTrit {
                        layer,
                        row,
                        col,
                        to,
                    });
                }
            }
        }
        if let Some(from) = target_layer.bias_trit(row) {
            let to = mode.target(from);
            if to != from {
                edits.push(Edit::SetBias { layer, row, to });
            }
        }
    }
    Ok(edits)
}

/// 算一组编辑的联合影响与单点影响。
pub fn impact(net: &Net, oracle: &Oracle, edits: &[Edit]) -> Result<ImpactReport> {
    oracle.check_arch(net)?;
    let want = oracle.outputs();
    let before = oracle.outputs_of(net)?;
    let before_loss = mismatch(&before, want);
    let after = apply(net, oracle, edits)?;
    let after_loss = mismatch(&after, want);
    let (lost, gained, changed) = diff(&before, &after, want);
    let mut individual = Vec::with_capacity(edits.len());
    let mut sum_gain = 0isize;
    for edit in edits {
        let out = apply(net, oracle, std::slice::from_ref(edit))?;
        let (item_lost, item_gained, item_changed) = diff(&before, &out, want);
        let item_gain = before_loss as isize - mismatch(&out, want) as isize;
        sum_gain += item_gain;
        individual.push(Item {
            edit: edit.clone(),
            gain: item_gain,
            lost: item_lost,
            gained: item_gained,
            changed: item_changed,
        });
    }
    let gain = before_loss as isize - after_loss as isize;
    Ok(ImpactReport {
        before_loss,
        after_loss,
        gain,
        lost,
        gained,
        changed,
        individual,
        synergy: gain - sum_gain,
    })
}

fn apply(net: &Net, oracle: &Oracle, edits: &[Edit]) -> Result<Vec<Vec<Trit>>> {
    let mut trial = net.clone();
    Patch::new(edits.to_vec()).apply(&mut trial)?;
    oracle.outputs_of(&trial)
}

fn mismatch(outputs: &[Vec<Trit>], want: &[Vec<Trit>]) -> usize {
    outputs
        .iter()
        .zip(want)
        .filter(|(got, expected)| got != expected)
        .count()
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
