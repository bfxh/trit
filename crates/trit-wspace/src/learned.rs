//! 学习式单点编辑提议器：线性打分器 + 静态/行为特征。
//!
//! 这是“模型提议、库复核”里的**模型一侧**：不调用外部模型，只用工件本身与 oracle
//! 输入统计训练一个 f32 线性打分器，给每个单点编辑打分；调用方再用
//! [`mod@crate::analyze`] / [`mod@crate::repair`] 实测复核。训练标签来自 `analyze` 的真实
//! `gain`，特征不读取 `after_loss`（避免标签泄漏）。
//!
//! 特征（16 个，顺序固定）：
//! `layer_frac, row_frac, col_frac, to, current, is_bias, row_nonzero, row_absmean,
//! row_scale, row_bias, row_activation_mean, row_activation_active, col_input_mean,
//! col_input_nonzero, to_matches_bias, row_is_output`。

use serde_json::{Value, json};

use crate::analyze::{Analysis, edit_key};
use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::net::Net;
use crate::oracle::Oracle;
use crate::repair;

mod features;
mod gradient;
pub use gradient::gradient_scores;

use features::FeatureTable;

/// 模型 JSON 的 magic。
pub const MAGIC: &str = "TRIT-PROPOSER";
/// 模型 JSON 的版本。
pub const VERSION: u32 = 1;
/// 特征名（顺序即权重顺序）。
pub const FEATURE_NAMES: [&str; 16] = [
    "layer_frac",
    "row_frac",
    "col_frac",
    "to",
    "current",
    "is_bias",
    "row_nonzero",
    "row_absmean",
    "row_scale",
    "row_bias",
    "row_activation_mean",
    "row_activation_active",
    "col_input_mean",
    "col_input_nonzero",
    "to_matches_bias",
    "row_is_output",
];

/// 一个训练样本：特征 + 归一化收益（`gain / before_loss`）。
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// 16 维特征。
    pub features: Vec<f32>,
    /// 归一化收益，正数表示 edit 降低失配。
    pub target: f32,
}

/// 线性打分器。
#[derive(Clone, Debug, PartialEq)]
pub struct Scorer {
    /// 16 个权重。
    pub weights: Vec<f32>,
    /// 偏置。
    pub bias: f32,
    /// 训练样本数。
    pub samples: usize,
    /// 训练轮数。
    pub epochs: usize,
    /// 学习率。
    pub lr: f32,
}

/// 一条打分后的候选。
#[derive(Clone, Debug, PartialEq)]
pub struct ScoredEdit {
    /// 候选编辑。
    pub edit: Edit,
    /// 模型分数（越大越可能有效）。
    pub score: f32,
}

/// 训练/留出报告。
#[derive(Clone, Debug, PartialEq)]
pub struct TrainReport {
    /// 全部样本。
    pub samples: usize,
    /// 训练样本。
    pub train: usize,
    /// 留出样本。
    pub holdout: usize,
    /// 训练轮数。
    pub epochs: usize,
    /// 学习率。
    pub lr: f32,
    /// 训练集 MSE。
    pub train_mse: f32,
    /// 留出集 MSE。
    pub holdout_mse: f32,
    /// 留出集中 target > 0 的样本数。
    pub holdout_positive: usize,
    /// 留出 top-K 的 K。
    pub top_k: usize,
    /// 留出 top-K 中 target > 0 的条数。
    pub top_k_hit: usize,
}

impl Scorer {
    /// 打分：`weights · features + bias`。
    #[must_use]
    pub fn score(&self, features: &[f32]) -> f32 {
        self.weights
            .iter()
            .zip(features)
            .map(|(weight, feature)| weight * feature)
            .sum::<f32>()
            + self.bias
    }

    /// 序列化为 JSON。
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "magic": MAGIC,
            "version": VERSION,
            "features": FEATURE_NAMES.len(),
            "feature_names": FEATURE_NAMES,
            "weights": &self.weights,
            "bias": self.bias,
            "samples": self.samples,
            "epochs": self.epochs,
            "lr": self.lr,
        })
    }

    /// 从 JSON 解析；magic/版本/长度/有限值都校验。
    pub fn from_json(value: &Value) -> Result<Self> {
        let magic = value
            .get("magic")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Wire("模型缺 magic".into()))?;
        if magic != MAGIC {
            return Err(Error::Wire(format!("模型 magic `{magic}` 不符")));
        }
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Wire("模型缺 version".into()))?;
        if version != u64::from(VERSION) {
            return Err(Error::Wire(format!("模型版本 {version} 不受支持")));
        }
        let features = value
            .get("features")
            .and_then(Value::as_u64)
            .ok_or_else(|| Error::Wire("模型缺 features".into()))?;
        if features != FEATURE_NAMES.len() as u64 {
            return Err(Error::Wire(format!(
                "模型 features={features}，期望 {}",
                FEATURE_NAMES.len()
            )));
        }
        let raw = value
            .get("weights")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Wire("模型缺 weights".into()))?;
        if raw.len() != FEATURE_NAMES.len() {
            return Err(Error::Wire(format!(
                "模型 weights 长度 {}，期望 {}",
                raw.len(),
                FEATURE_NAMES.len()
            )));
        }
        let mut weights = Vec::with_capacity(raw.len());
        for item in raw {
            let number = item
                .as_f64()
                .ok_or_else(|| Error::Wire("模型 weights 必须是数字".into()))?;
            let number = number as f32;
            if !number.is_finite() {
                return Err(Error::Wire("模型 weights 含非有限值".into()));
            }
            weights.push(number);
        }
        let bias = value
            .get("bias")
            .and_then(Value::as_f64)
            .ok_or_else(|| Error::Wire("模型缺 bias".into()))?;
        let bias = bias as f32;
        if !bias.is_finite() {
            return Err(Error::Wire("模型 bias 非有限".into()));
        }
        let samples = value.get("samples").and_then(Value::as_u64).unwrap_or(0) as usize;
        let epochs = value.get("epochs").and_then(Value::as_u64).unwrap_or(0) as usize;
        let lr = value.get("lr").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        Ok(Self {
            weights,
            bias,
            samples,
            epochs,
            lr,
        })
    }
}

/// 从 `analyze` 的实测结果构造训练样本。
pub fn samples_from_analysis(
    net: &Net,
    oracle: &Oracle,
    analysis: &Analysis,
) -> Result<Vec<Sample>> {
    let table = FeatureTable::new(net, oracle)?;
    Ok(analysis
        .candidates
        .iter()
        .map(|impact| {
            let total = (analysis.before_loss + impact.after_loss).max(1) as f32;
            Sample {
                features: table.extract(&impact.edit),
                target: impact.gain as f32 / total,
            }
        })
        .collect())
}

/// 训练线性打分器（SGD + L2）。
#[must_use]
pub fn train(samples: &[Sample], epochs: usize, lr: f32, l2: f32) -> Scorer {
    let mut weights = vec![0.0f32; FEATURE_NAMES.len()];
    let mut bias = 0.0f32;
    for _ in 0..epochs {
        for sample in samples {
            let prediction = dot(&weights, &sample.features) + bias;
            let error = prediction - sample.target;
            for (weight, feature) in weights.iter_mut().zip(&sample.features) {
                *weight -= lr * (error * feature + l2 * *weight);
            }
            bias -= lr * error;
        }
    }
    Scorer {
        weights,
        bias,
        samples: samples.len(),
        epochs,
        lr,
    }
}

/// 按 `holdout` 比例划分训练/留出，训练并报告 top-K 命中。
#[must_use]
pub fn train_holdout(
    samples: &[Sample],
    holdout: f32,
    epochs: usize,
    lr: f32,
    seed: u64,
) -> (Scorer, TrainReport) {
    let holdout = holdout.clamp(0.0, 0.5);
    let mut rng = Rng::new(seed);
    let mut order: Vec<usize> = (0..samples.len()).collect();
    for index in (1..order.len()).rev() {
        let swap = (rng.next_u64() as usize) % (index + 1);
        order.swap(index, swap);
    }
    let holdout_count = ((samples.len() as f32) * holdout).round() as usize;
    let (holdout_indices, train_indices) = order.split_at(holdout_count);
    let train_samples: Vec<Sample> = train_indices
        .iter()
        .map(|&index| samples[index].clone())
        .collect();
    let holdout_samples: Vec<Sample> = holdout_indices
        .iter()
        .map(|&index| samples[index].clone())
        .collect();
    let scorer = train(&train_samples, epochs, lr, 1e-4);
    let mut ranked: Vec<&Sample> = holdout_samples.iter().collect();
    ranked.sort_by(|left, right| {
        scorer
            .score(&right.features)
            .total_cmp(&scorer.score(&left.features))
    });
    let top_k = 16usize.min(ranked.len());
    let top_k_hit = ranked[..top_k]
        .iter()
        .filter(|sample| sample.target > 0.0)
        .count();
    let holdout_positive = holdout_samples
        .iter()
        .filter(|sample| sample.target > 0.0)
        .count();
    let report = TrainReport {
        samples: samples.len(),
        train: train_samples.len(),
        holdout: holdout_samples.len(),
        epochs,
        lr,
        train_mse: mse(&scorer, &train_samples),
        holdout_mse: mse(&scorer, &holdout_samples),
        holdout_positive,
        top_k,
        top_k_hit,
    };
    (scorer, report)
}

/// 对全部单点候选打分并取 top-`limit`（`limit=0` 表示全部）。
pub fn propose(
    net: &Net,
    oracle: &Oracle,
    scorer: &Scorer,
    limit: usize,
) -> Result<Vec<ScoredEdit>> {
    let table = FeatureTable::new(net, oracle)?;
    let mut scored: Vec<ScoredEdit> = repair::candidates(net, false)
        .into_iter()
        .map(|edit| ScoredEdit {
            score: scorer.score(&table.extract(&edit)),
            edit,
        })
        .collect();
    scored.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| edit_key(&left.edit).cmp(&edit_key(&right.edit)))
    });
    if limit > 0 {
        scored.truncate(limit);
    }
    Ok(scored)
}

/// 梯度知情排序：用 STE 反向传播算 `∂L/∂w`，按 `-grad * (to - current)` 打分。
///
/// 前向与 `Net::readout` 一致：隐藏层 `sign(acc)`，输出层保留 `scale * acc`；
/// 损失是输出层 raw 与目标 trit（-1/0/+1）的平方误差。隐藏层 `sign` 的导数用
/// 直通估计器（STE，恒为 1）。
fn dot(weights: &[f32], features: &[f32]) -> f32 {
    weights
        .iter()
        .zip(features)
        .map(|(weight, feature)| weight * feature)
        .sum()
}

fn mse(scorer: &Scorer, samples: &[Sample]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    samples
        .iter()
        .map(|sample| {
            let error = scorer.score(&sample.features) - sample.target;
            error * error
        })
        .sum::<f32>()
        / samples.len() as f32
}

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
