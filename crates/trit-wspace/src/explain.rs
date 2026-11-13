//! 单样本解释：这个样本为什么错、哪些单点编辑能让它翻转/变对。
//!
//! 两段式：
//!
//! 1. 对全部候选只做**这一个样本**的前向（大网也便宜），算 `flips` / `fixes`；
//! 2. 按 `(fixes, flips, 坐标)` 取 top-K，对它们做整验证集复核，给出 `after_loss` / `gain`。
//!
//! `top=0` 表示全部候选都做整验证集复核（小网用）。

use std::cmp::Ordering;

use crate::analyze::edit_key;
use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::repair;
use crate::trit::Trit;

/// 单条编辑对某个样本的影响。
#[derive(Clone, Debug, PartialEq)]
pub struct SampleEditImpact {
    /// 候选编辑。
    pub edit: Edit,
    /// 该样本输出是否改变。
    pub flips: bool,
    /// 该样本是否由错变对。
    pub fixes: bool,
    /// 整验证集上的失配数（只有 top-K 复核过才有）。
    pub after_loss: Option<usize>,
    /// `before_loss - after_loss`（只有 top-K 复核过才有）。
    pub gain: Option<isize>,
}

/// 单样本解释结果。
#[derive(Clone, Debug, PartialEq)]
pub struct SampleExplanation {
    /// 样本下标。
    pub index: usize,
    /// 输入。
    pub input: Vec<Trit>,
    /// 期望输出。
    pub want: Vec<Trit>,
    /// 当前输出。
    pub got: Vec<Trit>,
    /// 整验证集当前失配数。
    pub before_loss: usize,
    /// 单样本评估过的候选数。
    pub evaluated: usize,
    /// 做过整验证集复核的候选数（等于返回条数）。
    pub fully_evaluated: usize,
    /// 按 `fixes` → `gain` → `flips` → 坐标排序的候选。
    pub candidates: Vec<SampleEditImpact>,
}

/// 解释一个样本：哪些单点编辑会让它翻转/变对，以及它们的整验证集影响。
pub fn explain_sample(
    net: &Net,
    oracle: &Oracle,
    index: usize,
    top: usize,
) -> Result<SampleExplanation> {
    oracle.check_arch(net)?;
    let len = oracle.len();
    if index >= len {
        return Err(Error::SampleIndex { index, len });
    }
    let input = oracle.inputs()[index].clone();
    let want = oracle.outputs()[index].clone();
    let got = net.forward(&input)?;
    let before_loss = oracle.loss(net)?;
    let candidates = repair::candidates(net, false);
    let evaluated = candidates.len();

    let mut preliminary = Vec::with_capacity(evaluated);
    for edit in candidates {
        let mut trial = net.clone();
        edit.apply_to(&mut trial)?;
        let after = trial.forward(&input)?;
        let flips = after != got;
        let fixes = got != want && after == want;
        preliminary.push((edit, flips, fixes));
    }
    preliminary.sort_by(|left, right| {
        right
            .2
            .cmp(&left.2)
            .then(right.1.cmp(&left.1))
            .then_with(|| edit_key(&left.0).cmp(&edit_key(&right.0)))
    });
    let full = if top == 0 {
        preliminary.len()
    } else {
        top.min(preliminary.len())
    };
    preliminary.truncate(full);

    let mut candidates = Vec::with_capacity(full);
    for (edit, flips, fixes) in preliminary {
        let mut trial = net.clone();
        edit.apply_to(&mut trial)?;
        let after_loss = oracle.loss(&trial)?;
        candidates.push(SampleEditImpact {
            edit,
            flips,
            fixes,
            after_loss: Some(after_loss),
            gain: Some(before_loss as isize - after_loss as isize),
        });
    }
    candidates.sort_by(rank);
    Ok(SampleExplanation {
        index,
        input,
        want,
        got,
        before_loss,
        evaluated,
        fully_evaluated: full,
        candidates,
    })
}

fn rank(left: &SampleEditImpact, right: &SampleEditImpact) -> Ordering {
    right
        .fixes
        .cmp(&left.fixes)
        .then_with(|| {
            right
                .gain
                .unwrap_or(isize::MIN)
                .cmp(&left.gain.unwrap_or(isize::MIN))
        })
        .then(right.flips.cmp(&left.flips))
        .then_with(|| edit_key(&left.edit).cmp(&edit_key(&right.edit)))
}
