//! 关联回忆的训练集成：把 [`memory`](crate::memory) 的「手工快权重」换成
//! **学出来的注意力块**（[`AttentionBlock`]）。
//!
//! 任务：序列 `[k1, v1, k2, v2, q]`，`q` 是 `k1` 或 `k2`，模型要在末位输出
//! 与 `q` 配对的那个 `v`。token 是 bipolar one-hot（`+1` 在自己位置、其余 `-1`），
//! 4 个键 + 4 个值共 8 维；「键 → 值」的配对由种子给定的置换固定。
//! 查询可指第一个或第二个键 ⇒ 「永远盯位置 1」不再可行，
//! 答案位置由内容决定。
//!
//! 这是「注意力/残差/FFN 的训练集成」的 M0：完整走
//! 前向（三值 QAT）→ MSE（只算末位）→ 全链 STE 反向 → SGD 动量 的闭环。
//! 边界：配对固定 ⇒ FFN 支路同样能背答案，`attention_on_value_position`
//! 只是「注意力有没有学会内容寻址」的读数，不作为注意力必要性的证明。

use trit_learn::{Activation, Ste, mse};

use crate::block::{AttentionBlock, BlockOptimizers};

/// 键的个数（值同样多；token 总数 = 2×这个数 = 模型维数）。
pub const KEYS: usize = 4;

/// 一条关联回忆样本。
#[derive(Debug, Clone, PartialEq)]
pub struct RecallSample {
    /// `T×dim` 行主序扁平序列：`[k_i, v_π(i), k_j, v_π(j), q]`。
    pub seq: Vec<f32>,
    /// 末位应有的目标 token。
    pub target: Vec<f32>,
    /// 配对值在序列里的位置（内容寻址读数用）。
    pub value_position: usize,
}
/// 关联回忆数据集（全部有序键对，`n·(n-1)` 条）。
#[derive(Debug, Clone, PartialEq)]
pub struct RecallDataset {
    /// token 维数（= 2×KEYS）。
    pub dim: usize,
    /// 序列长度。
    pub seq_len: usize,
    /// 样本。
    pub samples: Vec<RecallSample>,
}

fn token(dim: usize, index: usize) -> Vec<f32> {
    (0..dim)
        .map(|i| if i == index { 1.0 } else { -1.0 })
        .collect()
}

/// 生成数据集：`seed` 决定「键 → 值」的置换，样本集本身是确定的全排列。
pub fn recall_dataset(seed: u64) -> RecallDataset {
    let dim = 2 * KEYS;
    // Fisher–Yates：seed 驱动的确定性置换 π，键 i 配值 token(KEYS + π[i])。
    let mut state = seed | 1;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut perm: Vec<usize> = (0..KEYS).collect();
    for i in (1..KEYS).rev() {
        let j = (next() % (i as u64 + 1)) as usize;
        perm.swap(i, j);
    }
    let keys: Vec<Vec<f32>> = (0..KEYS).map(|i| token(dim, i)).collect();
    let values: Vec<Vec<f32>> = (0..KEYS).map(|i| token(dim, KEYS + i)).collect();
    let seq_len = 5;
    let mut samples = Vec::new();
    for first in 0..KEYS {
        for second in 0..KEYS {
            if second == first {
                continue;
            }
            // 序列本体固定：两个键值对。查询既可指第一个键（答案在位置 1），
            // 也可指第二个键（答案在位置 3）——「永远盯一个位置」不可行。
            let mut prefix = Vec::with_capacity(4 * dim);
            prefix.extend_from_slice(&keys[first]);
            prefix.extend_from_slice(&values[perm[first]]);
            prefix.extend_from_slice(&keys[second]);
            prefix.extend_from_slice(&values[perm[second]]);
            for (query, position) in [(first, 1usize), (second, 3usize)] {
                let mut seq = prefix.clone();
                seq.extend_from_slice(&keys[query]);
                samples.push(RecallSample {
                    seq,
                    target: values[perm[query]].clone(),
                    value_position: position,
                });
            }
        }
    }
    RecallDataset {
        dim,
        seq_len,
        samples,
    }
}

/// 训练超参。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecallTrainOptions {
    /// 全批 epoch 数。
    pub epochs: usize,
    /// 学习率。
    pub learning_rate: f32,
    /// SGD 动量。
    pub momentum: f32,
    /// FFN 隐层宽度。
    pub hidden: usize,
    /// 初始化与「键→值」置换共用的种子。
    pub seed: u64,
}

impl Default for RecallTrainOptions {
    fn default() -> Self {
        Self {
            epochs: 1200,
            learning_rate: 0.3,
            momentum: 0.9,
            hidden: 24,
            seed: 7,
        }
    }
}

/// 一次关联回忆训练的报告。
#[derive(Debug, Clone, PartialEq)]
pub struct RecallTrainReport {
    /// 样本数。
    pub samples: usize,
    /// token 维数。
    pub dim: usize,
    /// 序列长度。
    pub seq_len: usize,
    /// FFN 隐层宽度。
    pub hidden: usize,
    /// 实际 epoch 数。
    pub epochs: usize,
    /// 第一个 epoch 的平均损失。
    pub initial_loss: f32,
    /// 训练结束后的平均损失（eval 重跑）。
    pub final_loss: f32,
    /// 末位 top-1 召回率。
    pub recall_accuracy: f32,
    /// 查询行落在配对值位置上的平均注意力概率（0–1，内容寻址读数）。
    pub attention_on_value_position: f32,
}

/// 末位最大分量的下标（并列取最小下标）。
fn argmax(values: &[f32]) -> usize {
    let mut best = 0usize;
    for (index, &value) in values.iter().enumerate() {
        if value > values[best] {
            best = index;
        }
    }
    best
}

/// 在数据集上前向评估：平均损失、top-1 召回率、内容寻址读数。
pub fn evaluate(block: &mut AttentionBlock, data: &RecallDataset) -> (f32, f32, f32) {
    let t = data.seq_len;
    let d = data.dim;
    let n = data.samples.len() as f32;
    let mut loss = 0.0f32;
    let mut correct = 0usize;
    let mut attention = 0.0f32;
    for sample in &data.samples {
        let y = block.forward(&sample.seq);
        let last = &y[(t - 1) * d..t * d];
        let (sample_loss, _) = mse(last, &sample.target);
        loss += sample_loss;
        if argmax(last) == argmax(&sample.target) {
            correct += 1;
        }
        attention += block.attention()[(t - 1) * t + sample.value_position];
    }
    (loss / n, correct as f32 / n, attention / n)
}

/// 训练关联回忆并返回报告（前向始终是三值 QAT，没有「导出再量化」落差）。
pub fn train_recall(options: RecallTrainOptions) -> RecallTrainReport {
    assert!(options.epochs > 0, "epochs 必须至少 1");
    assert!(
        options.learning_rate.is_finite() && options.learning_rate > 0.0,
        "learning_rate 必须是正的有限值"
    );
    assert!(
        options.momentum.is_finite() && (0.0..1.0).contains(&options.momentum),
        "momentum 必须在 [0, 1)"
    );
    assert!(options.hidden > 0, "hidden 必须非零");
    let data = recall_dataset(options.seed);
    let dim = data.dim;
    let seq_len = data.seq_len;
    let count = data.samples.len();
    let mut block = AttentionBlock::new(
        dim,
        options.hidden,
        Activation::Tanh,
        Ste::Identity,
        options.seed,
    );
    let mut opts = BlockOptimizers::sgd(options.learning_rate, options.momentum);
    let mut initial_loss = f32::NAN;
    for epoch in 0..options.epochs {
        block.zero_grad();
        let mut total = 0.0f32;
        for sample in &data.samples {
            let y = block.forward(&sample.seq);
            let last = &y[(seq_len - 1) * dim..seq_len * dim];
            let (loss, grad) = mse(last, &sample.target);
            total += loss;
            // 只在末位回传梯度，批内取均值。
            let mut grad_out = vec![0.0f32; seq_len * dim];
            let mean = 1.0 / count as f32;
            for (slot, &g) in grad_out[(seq_len - 1) * dim..].iter_mut().zip(grad.iter()) {
                *slot = g * mean;
            }
            let _ = block.backward(&grad_out);
        }
        if epoch == 0 {
            initial_loss = total / count as f32;
        }
        block.update(&mut opts);
    }
    let (final_loss, accuracy, attention) = evaluate(&mut block, &data);
    RecallTrainReport {
        samples: count,
        dim,
        seq_len,
        hidden: options.hidden,
        epochs: options.epochs,
        initial_loss,
        final_loss,
        recall_accuracy: accuracy,
        attention_on_value_position: attention,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_shape_and_pairing() {
        let data = recall_dataset(7);
        assert_eq!(data.dim, 2 * KEYS);
        assert_eq!(data.seq_len, 5);
        // 全部有序键对 × 每对两个查询方向。
        assert_eq!(data.samples.len(), KEYS * (KEYS - 1) * 2);
        // 末位查询 = 序列里的某个键；目标 token 恰好一个 +1；值位置 ∈ {1, 3}。
        let d = data.dim;
        for sample in &data.samples {
            assert_eq!(sample.seq.len(), data.seq_len * d);
            assert!([1, 3].contains(&sample.value_position));
            let query = &sample.seq[4 * d..5 * d];
            let value = &sample.seq[sample.value_position * d..(sample.value_position + 1) * d];
            // 查询 token 是键（+1 位置 < KEYS），目标 token 是值（+1 位置 ≥ KEYS）。
            let query_index = query.iter().position(|&v| v == 1.0).unwrap();
            assert!(query_index < KEYS);
            let target_index = sample.target.iter().position(|&v| v == 1.0).unwrap();
            assert!(target_index >= KEYS);
            // 目标与「查询配对的值」是同一个 token。
            assert_eq!(target_index, value.iter().position(|&v| v == 1.0).unwrap());
        }
        // 同种子同置换。
        assert_eq!(recall_dataset(7), recall_dataset(7));
        assert_ne!(recall_dataset(7), recall_dataset(8));
    }

    #[test]
    fn recall_is_learned() {
        let report = train_recall(RecallTrainOptions::default());
        assert!(
            report.final_loss < report.initial_loss * 0.25,
            "损失没有充分下降：{} → {}",
            report.initial_loss,
            report.final_loss
        );
        // 0.7 = 17/24：超越函数已走 f64 内部（跨平台轨迹一致），但 f32 舍入
        // 边界情形仍可能让个别平台落到略差的盆地——留出余量，不做位级承诺。
        assert!(
            report.recall_accuracy >= 0.7,
            "召回率只有 {}",
            report.recall_accuracy
        );
        assert!(
            (0.0..=1.0).contains(&report.attention_on_value_position),
            "注意力读数越界：{}",
            report.attention_on_value_position
        );
    }

    #[test]
    fn training_is_deterministic() {
        let options = RecallTrainOptions {
            epochs: 120,
            ..RecallTrainOptions::default()
        };
        assert_eq!(train_recall(options), train_recall(options));
    }

    #[test]
    fn another_seed_still_learns() {
        let report = train_recall(RecallTrainOptions {
            seed: 0x5EED,
            ..RecallTrainOptions::default()
        });
        assert!(
            report.recall_accuracy >= 0.75,
            "seed 2 召回率 {}",
            report.recall_accuracy
        );
    }

    #[test]
    #[should_panic(expected = "epochs")]
    fn zero_epochs_is_rejected() {
        let _ = train_recall(RecallTrainOptions {
            epochs: 0,
            ..RecallTrainOptions::default()
        });
    }

    #[test]
    #[should_panic(expected = "learning_rate")]
    fn bad_learning_rate_is_rejected() {
        let _ = train_recall(RecallTrainOptions {
            learning_rate: f32::NAN,
            ..RecallTrainOptions::default()
        });
    }
}
