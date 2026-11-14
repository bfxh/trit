//! Adam 能不能把留出集推过冻结基线？——与 `classifier_holdout` 同一套切分。
//!
//! 背景：plain SGD 在 384 维 LayerNorm 特征上条件数太差，200 步训练集只到 0.40
//! （闭式解 0.89），留出集停在机会线。Adam 用二阶矩做逐参数缩放，对尺度不敏感。
//!
//! 同一数据/切分下比较三方：
//! 1. **冻结基线**：块不动，闭式岭回归只拟合头；
//! 2. **Adam 训练块+头**：小批量，看训练集能否收敛、留出集能否超过 1。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::classifier_head::ClassifierHead;
use trit_wspace::classifier_train::{AdamOptions, AdamState, train_classify_adam_with_state};
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, quad_labels};
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::forward_with;

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
const LAMBDA: f32 = 1e-2;
const BATCH: usize = 128;
const DEFAULT_STEPS: usize = 200;
const DEFAULT_LR_BLOCK: f32 = 1e-3;
const DEFAULT_LR_HEAD: f32 = 1e-2;
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
        .expect("用法: classifier_adam <model.safetensors> [steps] [lr_block] [lr_head]");
    let steps: usize = args
        .next()
        .map(|value| value.parse().expect("steps"))
        .unwrap_or(DEFAULT_STEPS);
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

    // 1) 冻结基线。
    let frozen = forward_with(&block, &samples, &context).expect("frozen features");
    let baseline =
        ClassifierHead::fit_ridge(&pick(&frozen, train), &train_y, LAMBDA).expect("ridge solve");
    let baseline_train = baseline
        .accuracy(&pick(&frozen, train), &train_y)
        .expect("accuracy");
    let baseline_test = baseline
        .accuracy(&pick(&frozen, test), &test_y)
        .expect("accuracy");

    // 2) Adam 小批量训练块 + 头。
    let train_x = pick(&samples, train);
    let test_x = pick(&samples, test);
    let mut head = ClassifierHead::zeros(CLASSES, dim).expect("head");
    // 跨小批量复用同一份 Adam 状态，否则动量每步归零。
    let mut state = AdamState::new(&block, &head);
    let query_before = block.attention.query.clone();
    let ff2_before = block.ff2.weight.clone();
    let options = AdamOptions {
        rounds: 1,
        lr_block,
        lr_head,
        ..AdamOptions::default()
    };
    let mut losses: Vec<f32> = Vec::with_capacity(steps);
    let mut batch_accuracies: Vec<f32> = Vec::with_capacity(steps);
    let mut cursor = 0usize;
    for _ in 0..steps {
        let batch_x: Vec<Vec<f32>> = (0..BATCH)
            .map(|offset| train_x[(cursor + offset) % train_x.len()].clone())
            .collect();
        let batch_y: Vec<usize> = (0..BATCH)
            .map(|offset| train_y[(cursor + offset) % train_y.len()])
            .collect();
        cursor = (cursor + BATCH) % train_x.len();
        let report = train_classify_adam_with_state(
            &mut block, &mut head, &batch_x, &context, &batch_y, &options, &mut state,
        )
        .expect("adam train");
        losses.extend(report.losses);
        batch_accuracies.extend(report.accuracies);
    }

    let trained_train_features = forward_with(&block, &train_x, &context).expect("train features");
    let trained_test_features = forward_with(&block, &test_x, &context).expect("test features");
    let trained_train = head
        .accuracy(&trained_train_features, &train_y)
        .expect("accuracy");
    let trained_test = head
        .accuracy(&trained_test_features, &test_y)
        .expect("accuracy");

    let step = steps.div_ceil(5).max(1);
    let trajectory: Vec<f32> = (0..steps)
        .step_by(step)
        .map(|index| batch_accuracies[index])
        .collect();

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "samples": SAMPLES,
            "context": CONTEXT,
            "train_samples": train.len(),
            "test_samples": test.len(),
            "classes": CLASSES,
            "steps": steps,
            "batch": BATCH,
            "lr_block": lr_block,
            "lr_head": lr_head,
            "optimizer": "adam",
            "adam_steps": state.steps(),
            "lambda": LAMBDA,
            "chance": 1.0 / CLASSES as f32,
            "label_histogram": histogram,
            "frozen_baseline": { "train": baseline_train, "test": baseline_test },
            "adam": { "train": trained_train, "test": trained_test },
            "loss_first": losses.first().copied().unwrap_or(0.0),
            "loss_last": losses.last().copied().unwrap_or(0.0),
            "batch_accuracy_trajectory": trajectory,
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
