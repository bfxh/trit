//! 编辑提议：对每个单点编辑**预测**失配变化，按收益排序。
//!
//! 这是“模型提议、库复核”的接口：模型可以读候选并挑一条应用（或交给
//! [`mod@crate::repair`] 继续搜），但候选里的 `after_loss` / `fixed` / `broken`
//! 都是**真算出来的**，不是打分估计。候选只含点编辑（改一个 trit 或偏置），
//! 因为这是可逐条审、可回滚的最小粒度。

use std::collections::HashSet;

use crate::edit::Edit;
use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::repair;

/// 单条编辑的实测影响。
#[derive(Clone, Debug, PartialEq)]
pub struct EditImpact {
    /// 候选编辑。
    pub edit: Edit,
    /// 编辑前的失配数。
    pub before_loss: usize,
    /// 编辑后的失配数（实测）。
    pub after_loss: usize,
    /// `before_loss - after_loss`；正数表示变好。
    pub gain: isize,
    /// 由错变对的样本数。
    pub fixed: usize,
    /// 由对变错的样本数。
    pub broken: usize,
    /// 输出发生变化的样本数（含错→错但值不同）。
    pub changed: usize,
}

/// 一次分析的结果。
#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    /// 分析前的失配数。
    pub before_loss: usize,
    /// 评估过的候选总数（不受 `limit` 影响）。
    pub evaluated: usize,
    /// 按收益排序后的候选（最多 `limit` 条；`limit=0` 表示全部）。
    pub candidates: Vec<EditImpact>,
}

/// 两条点编辑的实测影响。
#[derive(Clone, Debug, PartialEq)]
pub struct PairImpact {
    /// 第一条编辑。
    pub first: Edit,
    /// 第二条编辑。
    pub second: Edit,
    /// 编辑前的失配数。
    pub before_loss: usize,
    /// 两条都应用后的失配数（实测）。
    pub after_loss: usize,
    /// `before_loss - after_loss`。
    pub gain: isize,
    /// 由错变对的样本数。
    pub fixed: usize,
    /// 由对变错的样本数。
    pub broken: usize,
    /// 输出发生变化的样本数。
    pub changed: usize,
}

/// 两编辑分析结果。
#[derive(Clone, Debug, PartialEq)]
pub struct PairAnalysis {
    /// 分析前的失配数。
    pub before_loss: usize,
    /// 评估过的候选对数（不受 `limit` 影响）。
    pub evaluated: usize,
    /// 按收益排序后的候选对（最多 `limit` 条；`limit=0` 表示全部）。
    pub candidates: Vec<PairImpact>,
}

/// 三编辑分析结果。
#[derive(Clone, Debug, PartialEq)]
pub struct TripleAnalysis {
    /// 分析前的失配数。
    pub before_loss: usize,
    /// 评估过的候选组合数（不受 `limit` 影响）。
    pub evaluated: usize,
    /// 按收益排序后的候选组合（最多 `limit` 条；`limit=0` 表示全部）。
    pub candidates: Vec<TripleImpact>,
}

/// 三条点编辑的实测影响。
#[derive(Clone, Debug, PartialEq)]
pub struct TripleImpact {
    /// 第一条编辑。
    pub first: Edit,
    /// 第二条编辑。
    pub second: Edit,
    /// 第三条编辑。
    pub third: Edit,
    /// 编辑前的失配数。
    pub before_loss: usize,
    /// 三条都应用后的失配数（实测）。
    pub after_loss: usize,
    /// `before_loss - after_loss`。
    pub gain: isize,
    /// 由错变对的样本数。
    pub fixed: usize,
    /// 由对变错的样本数。
    pub broken: usize,
    /// 输出发生变化的样本数。
    pub changed: usize,
}

/// 枚举全部**不同坐标**的三点编辑组合，实测 `after_loss` 并排序。
///
/// 组合数 `C(C-1)(C-2)/6` 超过 `max_triples` 时直接报预算错误，不做静默截断。
pub fn analyze_triples(
    net: &Net,
    oracle: &Oracle,
    limit: usize,
    max_triples: usize,
) -> Result<TripleAnalysis> {
    oracle.check_arch(net)?;
    let before_outputs = oracle.outputs_of(net)?;
    let before_mismatches = mismatches(&before_outputs, oracle);
    let before_set: HashSet<usize> = before_mismatches.iter().copied().collect();
    let before_loss = before_mismatches.len();

    let candidates = repair::candidates(net);
    let count = candidates.len();
    let total = count
        .saturating_mul(count.saturating_sub(1))
        .saturating_mul(count.saturating_sub(2))
        / 6;
    if total > max_triples {
        return Err(crate::error::Error::CombinationBudget {
            depth: 3,
            combinations: total,
            limit: max_triples,
        });
    }

    let mut impacts = Vec::new();
    for (index, first) in candidates.iter().enumerate() {
        for (second_index, second) in candidates.iter().enumerate().skip(index + 1) {
            if first.same_coord(second) {
                continue;
            }
            for third in &candidates[second_index + 1..] {
                if first.same_coord(third) || second.same_coord(third) {
                    continue;
                }
                let mut trial = net.clone();
                first.apply_to(&mut trial)?;
                second.apply_to(&mut trial)?;
                third.apply_to(&mut trial)?;
                let after_outputs = oracle.outputs_of(&trial)?;
                let after_mismatches = mismatches(&after_outputs, oracle);
                let after_set: HashSet<usize> = after_mismatches.iter().copied().collect();
                let fixed = before_mismatches
                    .iter()
                    .filter(|index| !after_set.contains(index))
                    .count();
                let broken = after_mismatches
                    .iter()
                    .filter(|index| !before_set.contains(index))
                    .count();
                let changed = before_outputs
                    .iter()
                    .zip(&after_outputs)
                    .filter(|(left, right)| left != right)
                    .count();
                impacts.push(TripleImpact {
                    first: first.clone(),
                    second: second.clone(),
                    third: third.clone(),
                    before_loss,
                    after_loss: after_mismatches.len(),
                    gain: before_loss as isize - after_mismatches.len() as isize,
                    fixed,
                    broken,
                    changed,
                });
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
            .then(left.changed.cmp(&right.changed))
            .then_with(|| edit_key(&left.first).cmp(&edit_key(&right.first)))
            .then_with(|| edit_key(&left.second).cmp(&edit_key(&right.second)))
            .then_with(|| edit_key(&left.third).cmp(&edit_key(&right.third)))
    });
    if limit > 0 {
        impacts.truncate(limit);
    }
    Ok(TripleAnalysis {
        before_loss,
        evaluated,
        candidates: impacts,
    })
}
/// 枚举全部**不同坐标**的点编辑对，实测 `after_loss` 并排序。
///
/// `C×C-1/2` 超过 `max_pairs` 时直接报 [`crate::Error::PairBudget`]，
/// 不做“只挑一部分”的静默截断（那会让 top-K 不再是真 top-K）。
pub fn analyze_pairs(
    net: &Net,
    oracle: &Oracle,
    limit: usize,
    max_pairs: usize,
) -> Result<PairAnalysis> {
    oracle.check_arch(net)?;
    let before_outputs = oracle.outputs_of(net)?;
    let before_mismatches = mismatches(&before_outputs, oracle);
    let before_set: HashSet<usize> = before_mismatches.iter().copied().collect();
    let before_loss = before_mismatches.len();

    let candidates = repair::candidates(net);
    let total_pairs = candidates
        .len()
        .saturating_mul(candidates.len().saturating_sub(1))
        / 2;
    if total_pairs > max_pairs {
        return Err(crate::error::Error::PairBudget {
            pairs: total_pairs,
            limit: max_pairs,
        });
    }

    let mut impacts = Vec::new();
    for (index, first) in candidates.iter().enumerate() {
        for second in &candidates[index + 1..] {
            if first.same_coord(second) {
                continue;
            }
            let mut trial = net.clone();
            first.apply_to(&mut trial)?;
            second.apply_to(&mut trial)?;
            let after_outputs = oracle.outputs_of(&trial)?;
            let after_mismatches = mismatches(&after_outputs, oracle);
            let after_set: HashSet<usize> = after_mismatches.iter().copied().collect();
            let fixed = before_mismatches
                .iter()
                .filter(|index| !after_set.contains(index))
                .count();
            let broken = after_mismatches
                .iter()
                .filter(|index| !before_set.contains(index))
                .count();
            let changed = before_outputs
                .iter()
                .zip(&after_outputs)
                .filter(|(left, right)| left != right)
                .count();
            impacts.push(PairImpact {
                first: first.clone(),
                second: second.clone(),
                before_loss,
                after_loss: after_mismatches.len(),
                gain: before_loss as isize - after_mismatches.len() as isize,
                fixed,
                broken,
                changed,
            });
        }
    }
    let evaluated = impacts.len();
    impacts.sort_by(|left, right| {
        right
            .gain
            .cmp(&left.gain)
            .then(left.after_loss.cmp(&right.after_loss))
            .then(left.broken.cmp(&right.broken))
            .then(left.changed.cmp(&right.changed))
            .then_with(|| edit_key(&left.first).cmp(&edit_key(&right.first)))
            .then_with(|| edit_key(&left.second).cmp(&edit_key(&right.second)))
    });
    if limit > 0 {
        impacts.truncate(limit);
    }
    Ok(PairAnalysis {
        before_loss,
        evaluated,
        candidates: impacts,
    })
}

/// 枚举全部点编辑，实测 `after_loss` 并排序。
///
/// 排序键：`gain` 降序 → `after_loss` 升序 → `broken` 升序 → `changed` 升序 →
/// 坐标字典序。最后一项保证同分时结果确定。
pub fn analyze(net: &Net, oracle: &Oracle, limit: usize) -> Result<Analysis> {
    oracle.check_arch(net)?;
    let before_outputs = oracle.outputs_of(net)?;
    let before_mismatches: Vec<usize> = mismatches(&before_outputs, oracle);
    let before_set: HashSet<usize> = before_mismatches.iter().copied().collect();
    let before_loss = before_mismatches.len();

    let candidates = repair::candidates(net);
    let evaluated = candidates.len();
    let mut impacts = Vec::with_capacity(evaluated);
    for edit in candidates {
        let mut trial = net.clone();
        edit.apply_to(&mut trial)?;
        let after_outputs = oracle.outputs_of(&trial)?;
        let after_mismatches = mismatches(&after_outputs, oracle);
        let after_set: HashSet<usize> = after_mismatches.iter().copied().collect();
        let fixed = before_mismatches
            .iter()
            .filter(|index| !after_set.contains(index))
            .count();
        let broken = after_mismatches
            .iter()
            .filter(|index| !before_set.contains(index))
            .count();
        let changed = before_outputs
            .iter()
            .zip(&after_outputs)
            .filter(|(left, right)| left != right)
            .count();
        impacts.push(EditImpact {
            edit,
            before_loss,
            after_loss: after_mismatches.len(),
            gain: before_loss as isize - after_mismatches.len() as isize,
            fixed,
            broken,
            changed,
        });
    }

    impacts.sort_by(|left, right| {
        right
            .gain
            .cmp(&left.gain)
            .then(left.after_loss.cmp(&right.after_loss))
            .then(left.broken.cmp(&right.broken))
            .then(left.changed.cmp(&right.changed))
            .then_with(|| edit_key(&left.edit).cmp(&edit_key(&right.edit)))
    });
    if limit > 0 {
        impacts.truncate(limit);
    }
    Ok(Analysis {
        before_loss,
        evaluated,
        candidates: impacts,
    })
}

fn mismatches(outputs: &[Vec<crate::trit::Trit>], oracle: &Oracle) -> Vec<usize> {
    outputs
        .iter()
        .zip(oracle.outputs())
        .enumerate()
        .filter_map(|(index, (got, want))| (got != want).then_some(index))
        .collect()
}

pub(crate) fn edit_key(edit: &Edit) -> (u8, usize, usize, usize, i64) {
    match *edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => (0, layer, row, col, i64::from(to.to_i8())),
        Edit::SetBias { layer, row, to } => (1, layer, row, 0, i64::from(to.to_i8())),
        Edit::SetScale { layer, row, to } => (2, layer, row, 0, i64::from(to.to_bits())),
        Edit::SwapRows { layer, a, b } => (3, layer, a, b, 0),
        Edit::NegateRow { layer, row } => (4, layer, row, 0, 0),
    }
}
