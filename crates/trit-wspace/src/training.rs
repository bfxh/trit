//! 训练过程/梯度历史：每轮修补后记录 loss 与输入侧敏感度变化。
//!
//! 三值网络没有反向传播梯度；这里用 [`perturb`] 的逐位翻转敏感度作为「梯度」的
//! 有限差分代理，记录它在每轮修补后的变化。

use crate::error::Result;
use crate::net::Net;
use crate::oracle::Oracle;
use crate::perturb::{PerturbMode, perturb};
use crate::repair::{RepairOptions, repair};

/// 一轮训练的记录。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoundRecord {
    /// 轮次。
    pub round: usize,
    /// 修补前失配数。
    pub before_loss: usize,
    /// 修补后失配数。
    pub after_loss: usize,
    /// 本轮补丁长度（0 = 没找到补丁）。
    pub patch_len: usize,
    /// 输入侧逐位翻转比例（「梯度」代理）。
    pub flip_rate: f64,
    /// 由对变错比例。
    pub lost_rate: f64,
    /// 最敏感输入位被翻转到多少样本。
    pub max_position_flips: usize,
}

/// 训练历史。
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    /// 每轮记录。
    pub rounds: Vec<RoundRecord>,
}

/// 跑 `rounds` 轮「修补 → 记录敏感度」。
pub fn run(
    net: &mut Net,
    oracle: &Oracle,
    rounds: usize,
    options: &RepairOptions,
) -> Result<History> {
    let mut history = Vec::new();
    for round in 0..rounds {
        let before_loss = oracle.loss(net)?;
        let found = repair(net, oracle, options)?;
        let patch_len = found.as_ref().map_or(0, |result| result.patch.len());
        if let Some(found) = &found {
            found.patch.apply(net)?;
        }
        let after_loss = oracle.loss(net)?;
        let report = perturb(net, oracle, PerturbMode::Flip)?;
        let width = report.width.max(1) as f64;
        let total = report.samples as f64 * width;
        let flip_rate = if total == 0.0 {
            0.0
        } else {
            report.flipped as f64 / total
        };
        let lost_rate = if total == 0.0 {
            0.0
        } else {
            report.lost as f64 / total
        };
        let max_position_flips = report
            .per_position
            .iter()
            .map(|position| position.flips)
            .max()
            .unwrap_or(0);
        history.push(RoundRecord {
            round,
            before_loss,
            after_loss,
            patch_len,
            flip_rate,
            lost_rate,
            max_position_flips,
        });
        if found.is_none() {
            break;
        }
    }
    Ok(History { rounds: history })
}
