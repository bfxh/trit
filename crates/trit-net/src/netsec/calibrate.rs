//! 分位数阈值校准：训练三值模型，并量出「弃权率 vs 误报率」曲线。
//!
//! 阈值不再手调常数：每个方向规则的下界/上界取**训练集该特征分位数的中间带**
//! `[P(0.5-b), P(0.5+b)]`，带内判 `X`。`b` 就是弃权旋钮——`b` 越大，判成
//! `Unknown` 的样本越多，曲线右移。
//!
//! 纪律：训练/测试按样本下标哈希切分，测试集只用来出读数；任何指标都带分母。

use trit_core::value::Ternary;

use super::dataset::{Label, Sample, quantile, sorted_by_feature};
use super::decision::{Verdict, decide};
use super::features::{DEFAULT_RULES, Rule, ternarize};
use crate::{DEFAULT_BUDGET, Layer, Model, TrainOptions, train};

/// 校准参数。
#[derive(Debug, Clone)]
pub struct Config {
    /// 弃权带宽度序列（0 表示硬阈值）。
    pub bands: Vec<f64>,
    /// 训练轮数。
    pub epochs: usize,
    /// 隐层单元数。
    pub hidden: usize,
    /// 随机种子。
    pub seed: u64,
    /// 训练集占比。
    pub train_fraction: f64,
    /// 训练集是否按类别下采样到 1:1。
    pub balance: bool,
    /// 学习率。
    pub learning_rate: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bands: vec![0.0, 0.1, 0.2, 0.3, 0.4],
            epochs: 120,
            hidden: 8,
            seed: 0x5eed_1234,
            train_fraction: 0.75,
            balance: true,
            learning_rate: 0.3,
        }
    }
}

/// 单个弃权带的读数。
#[derive(Debug, Clone)]
pub struct BandReport {
    /// 弃权带宽度。
    pub band: f64,
    /// 判 `X` 的分位下界。
    pub low_q: f64,
    /// 判 `X` 的分位上界。
    pub high_q: f64,
    /// 训练集样本数。
    pub train: usize,
    /// 测试集样本数。
    pub test: usize,
    /// 测试集攻击样本数。
    pub attack: usize,
    /// 给出确定结论的比例。
    pub coverage: f64,
    /// 弃权比例。
    pub abstain: f64,
    /// 已判样本里的误报率。
    pub fpr: f64,
    /// 已判攻击里的检出率。
    pub tpr: f64,
    /// 已判阳性里的精确率。
    pub precision: f64,
    /// 已判样本的准确率。
    pub accuracy: f64,
    /// 真阳性。
    pub tp: usize,
    /// 假阳性。
    pub fp: usize,
    /// 真阴性。
    pub tn: usize,
    /// 假阴性（判成良性）。
    pub fn_: usize,
    /// 攻击样本的弃权率。
    pub attack_abstain: f64,
    /// 良性样本的弃权率。
    pub benign_abstain: f64,
    /// 训练集最后一轮的全批损失。
    pub train_loss: f64,
    /// 导出后的三值模型在训练集上的精度。
    pub train_accuracy: f64,
    /// 影子（未量化）模型在训练集上的精度。
    pub shadow_accuracy: f64,
}

/// 切分下标：按下标哈希，确定性且不依赖输入顺序。
pub fn split(samples: &[Sample], fraction: f64, seed: u64) -> (Vec<usize>, Vec<usize>) {
    let threshold = (fraction.clamp(0.0, 1.0) * 10_000.0) as u64;
    let mut train = Vec::new();
    let mut test = Vec::new();
    for index in 0..samples.len() {
        if mix(index as u64 ^ seed) % 10_000 < threshold {
            train.push(index);
        } else {
            test.push(index);
        }
    }
    (train, test)
}

/// 用训练集分位数造规则。
pub fn rules_from_quantiles(sorted: &[Vec<f64>], band: f64) -> Vec<Rule> {
    let low_q = (0.5 - band).clamp(0.0, 1.0);
    let high_q = (0.5 + band).clamp(0.0, 1.0);
    DEFAULT_RULES
        .iter()
        .zip(sorted.iter())
        .map(|(base, values)| Rule {
            feature: base.feature,
            low: quantile(values, low_q),
            high: quantile(values, high_q),
            bias: base.bias,
        })
        .collect()
}

/// 跑完整条曲线。
pub fn run(samples: &[Sample], config: &Config) -> Result<Vec<BandReport>, String> {
    if samples.is_empty() {
        return Err("数据集为空".to_string());
    }
    let (train_idx, test_idx) = split(samples, config.train_fraction, config.seed);
    if train_idx.is_empty() || test_idx.is_empty() {
        return Err(format!(
            "切分后训练/测试为空（train={} test={}）",
            train_idx.len(),
            test_idx.len()
        ));
    }
    let train_idx = if config.balance {
        balanced(train_idx, samples)
    } else {
        train_idx
    };
    let train_set: Vec<Sample> = train_idx.iter().map(|index| samples[*index]).collect();
    let sorted: Vec<Vec<f64>> = DEFAULT_RULES
        .iter()
        .map(|rule| sorted_by_feature(&train_set, rule.feature))
        .collect();

    let mut reports = Vec::with_capacity(config.bands.len());
    for band in &config.bands {
        let rules = rules_from_quantiles(&sorted, *band);
        let (model, report) = fit(&train_set, &rules, config)?;
        reports.push(evaluate(
            samples,
            &test_idx,
            &rules,
            &model,
            *band,
            train_set.len(),
            &report,
        )?);
    }
    Ok(reports)
}

/// 多数类下采样到少数类数量（等距取）。三值 STE 全批训练在不平衡数据上
/// 会塌成常数负例——实测良性占比 71.8% 时 TPR 恒为 0。
fn balanced(indices: Vec<usize>, samples: &[Sample]) -> Vec<usize> {
    let mut malicious = Vec::new();
    let mut benign = Vec::new();
    for index in indices {
        if samples[index].label == Label::Malicious {
            malicious.push(index);
        } else {
            benign.push(index);
        }
    }
    let target = malicious.len().min(benign.len());
    if target == 0 {
        return malicious.into_iter().chain(benign).collect();
    }
    let keep = |pool: Vec<usize>| -> Vec<usize> {
        let stride = pool.len().div_ceil(target);
        pool.into_iter().step_by(stride).take(target).collect()
    };
    let mut out = keep(malicious);
    out.extend(keep(benign));
    out
}

fn fit(
    train_set: &[Sample],
    rules: &[Rule],
    config: &Config,
) -> Result<(Model, crate::TrainReport), String> {
    let inputs: Vec<Vec<f32>> = train_set
        .iter()
        .map(|sample| {
            ternarize(&sample.features, rules)
                .iter()
                .map(|value| match value {
                    Ternary::False => -1.0,
                    Ternary::Unknown => 0.0,
                    Ternary::True => 1.0,
                })
                .collect()
        })
        .collect();
    let labels: Vec<f32> = train_set
        .iter()
        .map(|sample| {
            if sample.label == Label::Malicious {
                1.0
            } else {
                -1.0
            }
        })
        .collect();
    let mut model = init_model(config.hidden, config.seed);
    let report = train(
        &mut model,
        &inputs,
        &labels,
        TrainOptions {
            epochs: config.epochs,
            learning_rate: config.learning_rate,
            seed: config.seed.wrapping_add(1),
        },
    )
    .map_err(|error| error.to_string())?;
    Ok((model, report))
}

fn evaluate(
    samples: &[Sample],
    test: &[usize],
    rules: &[Rule],
    model: &Model,
    band: f64,
    train_len: usize,
    report: &crate::TrainReport,
) -> Result<BandReport, String> {
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut tn = 0usize;
    let mut fn_ = 0usize;
    let mut attack_abstain = 0usize;
    let mut benign_abstain = 0usize;
    let mut attack = 0usize;
    for index in test {
        let sample = &samples[*index];
        let inputs = ternarize(&sample.features, rules);
        let decision = decide(model, &inputs, DEFAULT_BUDGET).map_err(|error| error.to_string())?;
        let malicious = sample.label == Label::Malicious;
        if malicious {
            attack += 1;
        }
        match (decision.verdict, malicious) {
            (Verdict::Malicious, true) => tp += 1,
            (Verdict::Malicious, false) => fp += 1,
            (Verdict::Benign, false) => tn += 1,
            (Verdict::Benign, true) => fn_ += 1,
            (Verdict::Undetermined, true) => attack_abstain += 1,
            (Verdict::Undetermined, false) => benign_abstain += 1,
        }
    }
    let total = test.len();
    let decided = tp + fp + tn + fn_;
    let benign = total.saturating_sub(attack);
    let ratio = |numerator: usize, denominator: usize| {
        if denominator == 0 {
            0.0
        } else {
            numerator as f64 / denominator as f64
        }
    };
    Ok(BandReport {
        band,
        low_q: (0.5 - band).clamp(0.0, 1.0),
        high_q: (0.5 + band).clamp(0.0, 1.0),
        train: train_len,
        test: total,
        attack,
        coverage: ratio(decided, total),
        abstain: ratio(total - decided, total),
        fpr: ratio(fp, fp + tn),
        tpr: ratio(tp, tp + fn_),
        precision: ratio(tp, tp + fp),
        accuracy: ratio(tp + tn, decided),
        tp,
        fp,
        tn,
        fn_,
        attack_abstain: ratio(attack_abstain, attack),
        benign_abstain: ratio(benign_abstain, benign),
        train_loss: f64::from(report.final_loss),
        train_accuracy: f64::from(report.ternary_accuracy),
        shadow_accuracy: f64::from(report.shadow_accuracy),
    })
}

fn init_model(hidden: usize, seed: u64) -> Model {
    let mut rng = Rng { state: seed | 1 };
    let mut first_weights = Vec::with_capacity(hidden);
    for _ in 0..hidden {
        let mut row = Vec::with_capacity(super::flow::N);
        for _ in 0..super::flow::N {
            row.push(rng.next());
        }
        first_weights.push(row);
    }
    let mut second_weights = Vec::with_capacity(hidden);
    for _ in 0..hidden {
        second_weights.push(rng.next());
    }
    Model {
        layers: vec![
            Layer {
                inputs: super::flow::N,
                outputs: hidden,
                weights: first_weights,
                bias: vec![0; hidden],
            },
            Layer {
                inputs: hidden,
                outputs: 1,
                weights: vec![second_weights],
                bias: vec![0],
            },
        ],
    }
}

struct Rng {
    state: u64,
}

impl Rng {
    fn next(&mut self) -> i8 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        if self.state & 1 == 0 { -1 } else { 1 }
    }
}

fn mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
