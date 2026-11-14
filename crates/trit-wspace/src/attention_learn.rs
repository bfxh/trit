//! 从头学习三值位置编码：贪心翻转每头的位置表，让 dominant 匹配目标。
//!
//! 不走梯度：每轮试「头 × 位置 × 维度 × 目标值」的单 trit 改动，取匹配头数最高的一次。
//! 每头有独立的位置表，初始全零，学到的编码完全来自目标行为。

use crate::attention_multi::MultiHeadAttention;
use crate::attention_position::{PositionEncoding, forward_with_encodings};
use crate::error::{Error, Result};
use crate::trit::Trit;

/// 学习选项。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LearnOptions {
    /// 最大轮数。
    pub max_rounds: usize,
    /// 最多学习前多少个位置。
    pub max_positions: usize,
    /// 每轮最多试多少个维度。
    pub max_dims: usize,
}

impl Default for LearnOptions {
    fn default() -> Self {
        Self {
            max_rounds: 8,
            max_positions: 16,
            max_dims: 8,
        }
    }
}

/// 学习报告。
#[derive(Clone, Debug, PartialEq)]
pub struct LearnReport {
    /// 每头学到的位置表。
    pub tables: Vec<PositionEncoding>,
    /// 匹配的目标头数。
    pub matched: usize,
    /// 目标总数。
    pub total: usize,
    /// 实际轮数。
    pub rounds: usize,
    /// 是否全部匹配。
    pub converged: bool,
}

/// 贪心学习位置编码。
pub fn learn_positions(
    multi: &MultiHeadAttention,
    query: &[Trit],
    keys: &[Vec<Trit>],
    values: &[Vec<Trit>],
    targets: &[usize],
    options: &LearnOptions,
) -> Result<LearnReport> {
    let Some(first) = multi.heads.first() else {
        return Err(Error::Shape {
            what: "heads",
            expected: 1,
            got: 0,
        });
    };
    let head_dim = first.key.cols();
    let positions = keys.len().min(options.max_positions);
    let dims = head_dim.min(options.max_dims);
    let heads = multi.heads.len();
    let mut tables = vec![vec![vec![Trit::Zero; head_dim]; keys.len()]; heads];
    let mut matched = score(multi, query, keys, values, &tables, targets)?;
    let mut rounds = 0usize;
    while matched < targets.len() && rounds < options.max_rounds {
        let mut best: Option<(usize, usize, usize, Trit, usize)> = None;
        for head in 0..heads {
            for position in 0..positions {
                for dim in 0..dims {
                    for candidate in [Trit::Pos, Trit::Neg, Trit::Zero] {
                        if tables[head][position][dim] == candidate {
                            continue;
                        }
                        let mut trial = tables.clone();
                        trial[head][position][dim] = candidate;
                        let trial_score = score(multi, query, keys, values, &trial, targets)?;
                        if trial_score > matched
                            && best.is_none_or(|(_, _, _, _, best_score)| trial_score > best_score)
                        {
                            best = Some((head, position, dim, candidate, trial_score));
                        }
                    }
                }
            }
        }
        let Some((head, position, dim, candidate, best_score)) = best else {
            break;
        };
        tables[head][position][dim] = candidate;
        matched = best_score;
        rounds += 1;
    }
    let total = targets.len();
    let tables = tables
        .into_iter()
        .map(PositionEncoding::new)
        .collect::<Result<Vec<_>>>()?;
    Ok(LearnReport {
        tables,
        matched,
        total,
        rounds,
        converged: matched == total,
    })
}

fn score(
    multi: &MultiHeadAttention,
    query: &[Trit],
    keys: &[Vec<Trit>],
    values: &[Vec<Trit>],
    tables: &[Vec<Vec<Trit>>],
    targets: &[usize],
) -> Result<usize> {
    let encodings = tables
        .iter()
        .map(|table| PositionEncoding::new(table.clone()))
        .collect::<Result<Vec<_>>>()?;
    let traces = forward_with_encodings(multi, query, keys, values, &encodings)?;
    Ok(traces
        .iter()
        .zip(targets)
        .filter(|(trace, target)| trace.dominant == **target)
        .count())
}
