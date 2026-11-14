//! 训练 block 能不能把**留出集**准确率推过"冻结基线"？
//!
//! 前一示例（`classifier_probe`）量出：冻结 block + 闭式岭回归在留出集上只有 0.367，
//! 还不如原始词向量的 0.395。本示例用同一套切分做两件事：
//!
//! 1. **冻结基线**：块不动，闭式岭回归只拟合头 → 留出集准确率；
//! 2. **训练 block**：交叉熵梯度同时回传块与头 → 留出集准确率。
//!
//! 样本与 attention 上下文分开（`train_classify_with`）：否则 768 个样本就是 768×768
//! 注意力，跑不动；而且全批一轮就要 27 秒，所以用**小批量** SGD（每步 `BATCH` 个样本）。
//!
//! 学习率很敏感：`lr_head=0.5` 会在几十步内发散（logit 到几百、softmax 饱和），
//! `lr_head<=0.02` 则几乎不动。默认值取实测不炸也不停的那一档。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, quad_labels};
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::{ClassifyOptions, forward_with, train_classify_with};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const KEY: &str = "encoder.layer.0.attention.self.key.weight";
const VALUE: &str = "encoder.layer.0.attention.self.value.weight";
const LN1_W: &str = "encoder.layer.0.attention.output.LayerNorm.weight";
const LN1_B: &str = "encoder.layer.0.attention.output.LayerNorm.bias";
const FF1_W: &str = "encoder.layer.0.intermediate.dense.weight";
const FF1_B: &str = "encoder.layer.0.intermediate.dense.bias";
const FF2_W: &str = "encoder.layer.0.output.dense.weight";
const FF2_B: &str = "encoder.layer.0.output.dense.bias";
const LN2_W: &str = "encoder.layer.0.output.LayerNorm.weight";
const LN2_B: &str = "encoder.layer.0.output.LayerNorm.bias";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const SAMPLES: usize = 1024;
const CONTEXT: usize = 64;
const CLASSES: usize = 4;
const TRAIN_RATIO: f32 = 0.75;
const BATCH: usize = 128;
const LAMBDA: f32 = 1e-2;
const DEFAULT_ROUNDS: usize = 100;
const DEFAULT_LR_BLOCK: f32 = 5e-3;
const DEFAULT_LR_HEAD: f32 = 5e-2;
const SEED: u64 = 20261112;

fn mean_abs_delta(now: &[f32], before: &[f32]) -> f32 {
    if now.len() != before.len() {
        return f32::NAN;
    }
    now.iter()
        .zip(before)
        .map(|(now, before)| (now - before).abs())
        .sum::<f32>()
        / now.len().max(1) as f32
}

fn pick(features: &[Vec<f32>], indices: &[usize]) -> Vec<Vec<f32>> {
    indices
        .iter()
        .map(|index| features[*index].clone())
        .collect()
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("用法: classifier_holdout <model.safetensors> [rounds] [lr_block] [lr_head]");
    let rounds: usize = args
        .next()
        .map(|value| value.parse().expect("rounds"))
        .unwrap_or(DEFAULT_ROUNDS);
    let lr_block: f32 = args
        .next()
        .map(|value| value.parse().expect("lr_block"))
        .unwrap_or(DEFAULT_LR_BLOCK);
    let lr_head: f32 = args
        .next()
        .map(|value| value.parse().expect("lr_head"))
        .unwrap_or(DEFAULT_LR_HEAD);

    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let key = safetensors::read_tensor(&path, KEY).expect("key");
    let value = safetensors::read_tensor(&path, VALUE).expect("value");
    let ln1_w = safetensors::read_vector(&path, LN1_W).expect("ln1 weight");
    let ln1_b = safetensors::read_vector(&path, LN1_B).expect("ln1 bias");
    let ff1_w = safetensors::read_tensor(&path, FF1_W).expect("ff1 weight");
    let ff1_b = safetensors::read_vector(&path, FF1_B).expect("ff1 bias");
    let ff2_w = safetensors::read_tensor(&path, FF2_W).expect("ff2 weight");
    let ff2_b = safetensors::read_vector(&path, FF2_B).expect("ff2 bias");
    let ln2_w = safetensors::read_vector(&path, LN2_W).expect("ln2 weight");
    let ln2_b = safetensors::read_vector(&path, LN2_B).expect("ln2 bias");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let mut block = TransformerBlock::new(
        F32Attention::new(query.values, key.values, value.values, dim, dim).expect("attention"),
        LayerNorm::new(ln1_w, ln1_b).expect("ln1"),
        Dense::new(ff1_w.values, ff1_b, ff1_w.rows, ff1_w.cols).expect("ff1"),
        Dense::new(ff2_w.values, ff2_b, ff2_w.rows, ff2_w.cols).expect("ff2"),
        LayerNorm::new(ln2_w, ln2_b).expect("ln2"),
    )
    .expect("block");

    // 样本 = 前 SAMPLES 行；上下文 = 其后 CONTEXT 行（与样本不重叠）。
    let samples: Vec<Vec<f32>> = (0..SAMPLES)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();
    let context: Vec<Vec<f32>> = (SAMPLES..SAMPLES + CONTEXT)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();

    let mut rng = Lcg::new(SEED);
    let labels = quad_labels(&mut rng, &samples, CLASSES);
    let mut histogram = vec![0usize; CLASSES];
    for label in &labels {
        histogram[*label] += 1;
    }

    let mut order: Vec<usize> = (0..SAMPLES).collect();
    for index in (1..SAMPLES).rev() {
        let draw = rng.next_f32().abs();
        let swap = (draw * (index + 1) as f32) as usize % (index + 1);
        order.swap(index, swap);
    }
    let split = (SAMPLES as f32 * TRAIN_RATIO) as usize;
    let (train, test) = order.split_at(split);
    let train_y: Vec<usize> = train.iter().map(|index| labels[*index]).collect();
    let test_y: Vec<usize> = test.iter().map(|index| labels[*index]).collect();

    // 1) 冻结基线：块不动，闭式岭回归只拟合头。
    let frozen = forward_with(&block, &samples, &context).expect("frozen features");
    let baseline =
        ClassifierHead::fit_ridge(&pick(&frozen, train), &train_y, LAMBDA).expect("ridge solve");
    let baseline_train = baseline
        .accuracy(&pick(&frozen, train), &train_y)
        .expect("accuracy");
    let baseline_test = baseline
        .accuracy(&pick(&frozen, test), &test_y)
        .expect("accuracy");

    // 2) 训练 block + 头。
    let train_x = pick(&samples, train);
    let test_x = pick(&samples, test);
    let mut head = ClassifierHead::zeros(CLASSES, dim).expect("head");
    let query_before = block.attention.query.clone();
    let ff2_before = block.ff2.weight.clone();
    // 小批量 SGD：全批一轮要几十秒，改成每步 BATCH 个样本、循环取样。
    let mut losses: Vec<f32> = Vec::with_capacity(rounds);
    let mut accuracies: Vec<f32> = Vec::with_capacity(rounds);
    let mut cursor = 0usize;
    for _ in 0..rounds {
        let batch_x: Vec<Vec<f32>> = (0..BATCH)
            .map(|offset| train_x[(cursor + offset) % train_x.len()].clone())
            .collect();
        let batch_y: Vec<usize> = (0..BATCH)
            .map(|offset| train_y[(cursor + offset) % train_y.len()])
            .collect();
        cursor = (cursor + BATCH) % train_x.len();
        let step = train_classify_with(
            &mut block,
            &mut head,
            &batch_x,
            &context,
            &batch_y,
            &ClassifyOptions {
                rounds: 1,
                lr_block,
                lr_head,
            },
        )
        .expect("train");
        losses.extend(step.losses);
        accuracies.extend(step.accuracies);
    }

    let trained_train_features = forward_with(&block, &train_x, &context).expect("train features");
    let trained_test_features = forward_with(&block, &test_x, &context).expect("test features");
    let trained_train = head
        .accuracy(&trained_train_features, &train_y)
        .expect("accuracy");
    let trained_test = head
        .accuracy(&trained_test_features, &test_y)
        .expect("accuracy");

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "samples": SAMPLES,
            "context": CONTEXT,
            "train_samples": train.len(),
            "test_samples": test.len(),
            "classes": CLASSES,
            "rounds": rounds,
            "lr_block": lr_block,
            "lr_head": lr_head,
            "lambda": LAMBDA,
            "chance": 1.0 / CLASSES as f32,
            "label_histogram": histogram,
            "frozen_baseline": { "train": baseline_train, "test": baseline_test },
            "trained": { "train": trained_train, "test": trained_test },
            "loss_first": losses.first().copied().unwrap_or(0.0),
            "loss_last": losses.last().copied().unwrap_or(0.0),
            "batch_accuracy_last": accuracies.last().copied().unwrap_or(0.0),
            "block_delta": {
                "query": mean_abs_delta(&block.attention.query, &query_before),
                "ff2": mean_abs_delta(&block.ff2.weight, &ff2_before),
            },
            "finite": losses.iter().all(|loss| loss.is_finite())
                && trained_test_features.iter().flatten().all(|value| value.is_finite()),
        }))
        .expect("json")
    );
}
