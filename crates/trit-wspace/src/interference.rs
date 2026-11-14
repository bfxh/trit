//! 批量编辑的**成对干扰矩阵**与贪心重排。
//!
//! 单条编辑的收益不可加：两条各自都有用，合起来可能互相抵消，也可能协同。
//! 这里把 base 上的 k 条候选编辑逐个、成对地**实测**，输出 k×k 交互项与一个贪心顺序：
//!
//! ```text
//! gain_i  = loss(base) - loss(base + edit_i)
//! gain_ij = loss(base) - loss(base + edit_i + edit_j)
//! interface[i][j] = gain_ij - (gain_i + gain_j)   // >0 协同，<0 冲突
//! ```
//!
//! 贪心重排每一步都选「加入后 loss 最小」的那条，并记录边际变化；它回答的是
//! 「一次提交 k 条时该按什么顺序落」，不是「哪条单独最好」。

use serde_json::{Value, json};

use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;

/// 贪心重排的一步。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GreedyStep {
    /// 选中的编辑下标。
    pub index: usize,
    /// 加入后的 loss。
    pub loss: usize,
    /// 相对上一步的 loss 变化（负 = 改善）。
    pub delta: isize,
}

impl GreedyStep {
    fn to_json(self) -> Value {
        json!({"index": self.index, "loss": self.loss, "delta": self.delta})
    }
}

/// 一批候选编辑的干扰分析结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterferenceReport {
    /// base 上的 loss。
    pub base_loss: usize,
    /// 每条编辑单独应用后的 loss（按输入顺序）。
    pub singles: Vec<usize>,
    /// `base_loss - singles[i]`（正 = 有改善）。
    pub gains: Vec<isize>,
    /// k×k 交互项（对称，对角线为 0）。
    pub matrix: Vec<Vec<isize>>,
    /// 贪心顺序与每步 loss。
    pub greedy: Vec<GreedyStep>,
    /// 贪心全选后的 loss。
    pub greedy_loss: usize,
}

impl InterferenceReport {
    /// → JSON（矩阵按输入顺序展开）。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "base_loss": self.base_loss,
            "singles": self.singles,
            "gains": self.gains,
            "matrix": self.matrix,
            "greedy": self.greedy.iter().map(|step| step.to_json()).collect::<Vec<_>>(),
            "greedy_loss": self.greedy_loss,
        })
    }
}

/// 实测 k 条编辑的单条收益、成对交互项与贪心顺序。
pub fn analyze_interference(
    base: &Net,
    oracle: &Oracle,
    edits: &[Edit],
) -> Result<InterferenceReport> {
    if edits.is_empty() {
        return Err(Error::Wire("干扰矩阵需要至少一条编辑".into()));
    }
    let base_loss = oracle.loss(base)?;
    let count = edits.len();

    let mut singles = Vec::with_capacity(count);
    for edit in edits {
        let mut net = base.clone();
        edit.apply_to(&mut net)?;
        singles.push(oracle.loss(&net)?);
    }
    let gains: Vec<isize> = singles
        .iter()
        .map(|loss| base_loss as isize - *loss as isize)
        .collect();

    let mut matrix = vec![vec![0isize; count]; count];
    for left in 0..count {
        for right in (left + 1)..count {
            let mut net = base.clone();
            edits[left].apply_to(&mut net)?;
            edits[right].apply_to(&mut net)?;
            let pair_loss = oracle.loss(&net)?;
            let pair_gain = base_loss as isize - pair_loss as isize;
            let interaction = pair_gain - (gains[left] + gains[right]);
            matrix[left][right] = interaction;
            matrix[right][left] = interaction;
        }
    }

    let mut current = base.clone();
    let mut current_loss = base_loss;
    let mut remaining: Vec<usize> = (0..count).collect();
    let mut greedy = Vec::with_capacity(count);
    while !remaining.is_empty() {
        let mut best: Option<(usize, usize)> = None;
        for &index in &remaining {
            let mut trial = current.clone();
            edits[index].apply_to(&mut trial)?;
            let loss = oracle.loss(&trial)?;
            if best.is_none_or(|(_, best_loss)| loss < best_loss) {
                best = Some((index, loss));
            }
        }
        let (index, loss) = best.expect("remaining 非空");
        greedy.push(GreedyStep {
            index,
            loss,
            delta: loss as isize - current_loss as isize,
        });
        edits[index].apply_to(&mut current)?;
        current_loss = loss;
        remaining.retain(|value| *value != index);
    }

    Ok(InterferenceReport {
        base_loss,
        singles,
        gains,
        matrix,
        greedy,
        greedy_loss: current_loss,
    })
}
