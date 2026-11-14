//! dimsweep 的列排序：决定稀疏化时先保留哪些列。
//!
//! 四种排序共用同一个 `prune_curve`，用来回答「`w_eff` 是不是已经是最优排序」。

use crate::basis::{Rng, Sample, criterion_direction};
use trit_wspace::Trit;
use trit_wspace::safetensors::Weights;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PruneOrder {
    /// 判据方向 `|w_eff|`：线性可加假设下的精确贡献。
    Weff,
    /// 输入列的方差：与标签无关，纯数据统计。
    Variance,
    /// Fisher 分数 `|μ₊−μ₋| / σ`：列与标签的相关性（互信息的线性代理）。
    Fisher,
    /// 固定种子的随机排列：下界基线。
    Random,
}

pub const ORDERS: [PruneOrder; 4] = [
    PruneOrder::Weff,
    PruneOrder::Variance,
    PruneOrder::Fisher,
    PruneOrder::Random,
];

impl PruneOrder {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            PruneOrder::Weff => "weff",
            PruneOrder::Variance => "variance",
            PruneOrder::Fisher => "fisher",
            PruneOrder::Random => "random",
        }
    }
}

/// 按给定排序策略返回列索引（重要性降序）。
pub fn order(
    kind: PruneOrder,
    samples: &[Sample],
    query: &Weights,
    readout: &[f32],
    dim: usize,
) -> Vec<usize> {
    let mut score = vec![0.0f64; dim];
    match kind {
        PruneOrder::Weff => {
            let w = criterion_direction(query, readout, dim);
            for (j, value) in w.iter().enumerate() {
                score[j] = value.abs();
            }
        }
        PruneOrder::Variance => {
            for sample in samples {
                for (j, value) in sample.x.iter().enumerate() {
                    let x = f64::from(*value);
                    score[j] += x * x;
                }
            }
        }
        PruneOrder::Fisher => {
            let mut n_pos = 0.0f64;
            let mut n_neg = 0.0f64;
            let mut mean_pos = vec![0.0f64; dim];
            let mut mean_neg = vec![0.0f64; dim];
            for sample in samples {
                let slot = if sample.teacher == Trit::Pos {
                    n_pos += 1.0;
                    &mut mean_pos
                } else {
                    n_neg += 1.0;
                    &mut mean_neg
                };
                for (j, value) in sample.x.iter().enumerate() {
                    slot[j] += f64::from(*value);
                }
            }
            for j in 0..dim {
                if n_pos > 0.0 {
                    mean_pos[j] /= n_pos;
                }
                if n_neg > 0.0 {
                    mean_neg[j] /= n_neg;
                }
            }
            let mut var = vec![0.0f64; dim];
            for sample in samples {
                let mean = if sample.teacher == Trit::Pos {
                    &mean_pos
                } else {
                    &mean_neg
                };
                for (j, value) in sample.x.iter().enumerate() {
                    let d = f64::from(*value) - mean[j];
                    var[j] += d * d;
                }
            }
            for j in 0..dim {
                let sigma = (var[j] / samples.len().max(1) as f64).sqrt().max(1e-9);
                score[j] = (mean_pos[j] - mean_neg[j]).abs() / sigma;
            }
        }
        PruneOrder::Random => {
            let mut rng = Rng::new(0x5EED_1234);
            for slot in &mut score {
                *slot = (rng.next() % 100_000) as f64;
            }
        }
    }
    let mut idx: Vec<usize> = (0..dim).collect();
    idx.sort_by(|a, b| {
        score[*b]
            .partial_cmp(&score[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx
}
