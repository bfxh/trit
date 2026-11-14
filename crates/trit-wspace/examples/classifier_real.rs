//! 真实词向量上的分类：MiniLM 词向量 + 教师定义的 4 类标签。
//!
//! 每个 token 是一个样本；标签由教师（[`trit_wspace::teacher_f32`]）给出，学生 =
//! MiniLM 一层（attention + 2×LN + 2×FFN，全参数反向）+ 线性分类头，交叉熵训练。
//!
//! 两种教师：
//! - `linear`（默认）：单个随机投影取 argmax。块输出本就可线性解码，头单独就能学会。
//! - `quad`：二次型得分按分位切 4 档（每类样本数相同），非线性、线性头做不到。
//!
//! 这是受控合成标签，不是真实标注语料。**重要边界**：32 个样本、384 维特征、头 1536 个
//! 参数 ⇒ 线性头能记住任意标注，所以"准确率 1.0"不能当作收敛/泛化的证据（见 docs）。

use serde_json::json;
use trit_wspace::attention_f32::F32Attention;
use trit_wspace::safetensors;
use trit_wspace::teacher_f32::{Lcg, linear_labels, quad_labels};
use trit_wspace::transformer_f32::{Dense, LayerNorm, TransformerBlock};
use trit_wspace::transformer_f32_cls::{ClassifierHead, ClassifyOptions, train_classify};

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
const SAMPLES: usize = 32;
const CLASSES: usize = 4;
const DEFAULT_ROUNDS: usize = 200;
const DEFAULT_LR_BLOCK: f32 = 1e-4;
const DEFAULT_LR_HEAD: f32 = 1e-1;
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

fn accuracy(
    block: &TransformerBlock,
    head: &ClassifierHead,
    tokens: &[Vec<f32>],
    labels: &[usize],
) -> f32 {
    let outs = block.forward(tokens).expect("forward");
    let hits = outs
        .iter()
        .zip(labels)
        .filter(|(out, label)| head.predict(out).expect("predict") == **label)
        .count();
    hits as f32 / labels.len() as f32
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect(
        "用法: classifier_real <model.safetensors> [rounds] [lr_block] [lr_head] [linear|quad]",
    );
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
    let teacher_mode = args.next().unwrap_or_else(|| "linear".to_string());

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

    let tokens: Vec<Vec<f32>> = (0..SAMPLES)
        .map(|row| embeddings.values[row * dim..(row + 1) * dim].to_vec())
        .collect();

    // 教师与分类头共用一条随机流，保证读数可复现（抽取顺序是契约）。
    let mut rng = Lcg::new(SEED);
    let labels: Vec<usize> = match teacher_mode.as_str() {
        "linear" => linear_labels(&mut rng, &tokens, CLASSES),
        "quad" => quad_labels(&mut rng, &tokens, CLASSES),
        other => panic!("未知教师: {other}（linear | quad）"),
    };
    let mut histogram = vec![0usize; CLASSES];
    for label in &labels {
        histogram[*label] += 1;
    }
    let majority = histogram.iter().copied().max().unwrap_or(0) as f32 / SAMPLES as f32;

    // 分类头用小随机初始化，避免「全零头」让初始准确率退化成多数类比例。
    let mut head = ClassifierHead::new(
        (0..CLASSES * dim).map(|_| rng.next_f32() * 0.01).collect(),
        vec![0.0; CLASSES],
        CLASSES,
        dim,
    )
    .expect("head");
    let before = accuracy(&block, &head, &tokens, &labels);
    let query_before = block.attention.query.clone();
    let ff2_before = block.ff2.weight.clone();
    let head_before = head.weight().to_vec();

    let report = train_classify(
        &mut block,
        &mut head,
        &tokens,
        &labels,
        &ClassifyOptions {
            rounds,
            lr_block,
            lr_head,
        },
    )
    .expect("train");
    let after = accuracy(&block, &head, &tokens, &labels);

    let step = rounds.div_ceil(5).max(1);
    let trajectory: Vec<(usize, f32)> = (0..rounds)
        .step_by(step)
        .map(|round| (round + 1, report.accuracies[round]))
        .collect();

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "dim": dim,
            "samples": SAMPLES,
            "classes": CLASSES,
            "rounds": rounds,
            "lr_block": lr_block,
            "lr_head": lr_head,
            "teacher": teacher_mode,
            "label_histogram": histogram,
            "majority_baseline": majority,
            "chance": 1.0 / CLASSES as f32,
            "accuracy_before": before,
            "accuracy_after": after,
            "loss_first": report.losses.first().copied().unwrap_or(0.0),
            "loss_last": report.losses.last().copied().unwrap_or(0.0),
            "block_delta": {
                "query": mean_abs_delta(&block.attention.query, &query_before),
                "ff2": mean_abs_delta(&block.ff2.weight, &ff2_before),
                "head": mean_abs_delta(head.weight(), &head_before),
            },
            "accuracy_trajectory": trajectory
                .iter()
                .map(|(round, value)| json!({ "round": round, "accuracy": value }))
                .collect::<Vec<_>>(),
            "finite": report.losses.iter().all(|loss| loss.is_finite())
                && block.ff2.weight.iter().all(|weight| weight.is_finite()),
        }))
        .expect("json")
    );
}
