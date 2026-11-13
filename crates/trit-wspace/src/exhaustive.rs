//! 形式化的不可修性：对给定验证集穷举全部 ≤1/≤2 编辑。
//!
//! `repair` 的束搜索是启发式；这里改用 [`analyze`] / [`analyze_pairs`] 的**全枚举**，
//! 给出「在给定验证集上不存在 ≤N 步解」的确定性结论（仍不是全部可能输入的证明）。

use crate::analyze::{analyze, analyze_pairs};
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;

/// 穷举结论。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExhaustiveReport {
    /// 允许的最大编辑数（0 / 1 / 2）。
    pub max_edits: usize,
    /// 是否存在 ≤N 步解。
    pub found: bool,
    /// 实际枚举的候选数。
    pub evaluated: usize,
    /// 所有候选里的最小失配数。
    pub best_after_loss: usize,
}

/// 穷举 ≤`max_edits` 步的所有候选。
pub fn exhaustive(
    net: &Net,
    oracle: &Oracle,
    max_edits: usize,
    max_pairs: usize,
) -> Result<ExhaustiveReport> {
    oracle.check_arch(net)?;
    let before_loss = oracle.loss(net)?;
    match max_edits {
        0 => Ok(ExhaustiveReport {
            max_edits: 0,
            found: before_loss == 0,
            evaluated: 0,
            best_after_loss: before_loss,
        }),
        1 => {
            let analysis = analyze(net, oracle, 0)?;
            let best = analysis
                .candidates
                .first()
                .map_or(before_loss, |item| item.after_loss);
            Ok(ExhaustiveReport {
                max_edits: 1,
                found: best == 0,
                evaluated: analysis.evaluated,
                best_after_loss: best,
            })
        }
        2 => {
            let pairs = analyze_pairs(net, oracle, 0, max_pairs)?;
            let best = pairs
                .candidates
                .first()
                .map_or(before_loss, |item| item.after_loss);
            Ok(ExhaustiveReport {
                max_edits: 2,
                found: best == 0,
                evaluated: pairs.evaluated,
                best_after_loss: best,
            })
        }
        other => Err(Error::Wire(format!(
            "--max-edits 只支持 0/1/2，得到 {other}"
        ))),
    }
}
