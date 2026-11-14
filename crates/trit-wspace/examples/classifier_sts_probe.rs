//! 冻结 6 层表示的**线性上限**：闭式岭回归 + 5 折交叉验证 + 特征缓存。
//!
//! 目的：在投入"训练 block"（需要为完整 BERT 层补反向传播）之前，先量"冻结表示 +
//! 训练一个线性头"能到多少。
//!
//! - 余弦基线（不训练）在 test 全量是 **Spearman 0.8204**（`classifier_sts_bert`）；
//! - 这里用 `[u; v]`（两句话向量拼接，768 维）岭回归预测 STS 分数，5 折交叉验证；
//! - 扫 λ 与"是否标准化"，因为余弦是**归一化交互**（含除法），线性头未必学得到。
//!
//! 特征提取一次约 20 分钟，所以支持缓存（`STS_FEATURE_CACHE` 指向文件），
//! 之后扫超参是秒级。
//!
//! 注意：在 test 集上做交叉验证只用于估计上限，不是官方协议。

use serde_json::json;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::{Dense, LayerNorm};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";
const CLS_ID: usize = 101;
const SEP_ID: usize = 102;
const HEADS: usize = 12;
const FOLDS: usize = 5;
const LAMBDAS: [f64; 5] = [0.01, 1.0, 100.0, 10_000.0, 1_000_000.0];

#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use common::{Sentence, cosine, encode, gelu, load_vocab, read_pairs, spearman};

fn layer_key(layer: usize, suffix: &str) -> String {
    format!("encoder.layer.{layer}.{suffix}")
}

fn dense(path: &str, name: &str) -> Dense {
    let weight = safetensors::read_tensor(path, name).expect("dense weight");
    let bias = safetensors::read_vector(path, &name.replace(".weight", ".bias")).expect("bias");
    Dense::new(weight.values, bias, weight.rows, weight.cols).expect("dense")
}

fn layernorm(path: &str, name: &str) -> LayerNorm {
    let weight = safetensors::read_vector(path, &name.replace(".bias", ".weight")).expect("ln w");
    let bias = safetensors::read_vector(path, name).expect("ln b");
    LayerNorm::new(weight, bias).expect("ln")
}

/// 完整的一层 BERT（12 头、GELU、post-LN），与 `classifier_sts_bert` 同源。
struct BertLayer {
    wq: Dense,
    wk: Dense,
    wv: Dense,
    wo: Dense,
    ln1: LayerNorm,
    ff1: Dense,
    ff2: Dense,
    ln2: LayerNorm,
}

impl BertLayer {
    fn load(path: &str, layer: usize) -> Self {
        Self {
            wq: dense(path, &layer_key(layer, "attention.self.query.weight")),
            wk: dense(path, &layer_key(layer, "attention.self.key.weight")),
            wv: dense(path, &layer_key(layer, "attention.self.value.weight")),
            wo: dense(path, &layer_key(layer, "attention.output.dense.weight")),
            ln1: layernorm(path, &layer_key(layer, "attention.output.LayerNorm.bias")),
            ff1: dense(path, &layer_key(layer, "intermediate.dense.weight")),
            ff2: dense(path, &layer_key(layer, "output.dense.weight")),
            ln2: layernorm(path, &layer_key(layer, "output.LayerNorm.bias")),
        }
    }

    fn forward(&self, sequence: &Sentence) -> Sentence {
        let dim = sequence[0].len();
        let head_dim = dim / HEADS;
        let scale = 1.0 / (head_dim as f32).sqrt();
        let keys: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wk.forward(token, false))
            .collect();
        let values: Vec<Vec<f32>> = sequence
            .iter()
            .map(|token| self.wv.forward(token, false))
            .collect();
        let mut out = Vec::with_capacity(sequence.len());
        for token in sequence {
            let query = self.wq.forward(token, false);
            let mut pooled = vec![0.0f32; dim];
            for head in 0..HEADS {
                let offset = head * head_dim;
                let span = offset..offset + head_dim;
                let query_head = &query[span.clone()];
                let scores: Vec<f32> = keys
                    .iter()
                    .map(|key| {
                        query_head
                            .iter()
                            .zip(&key[span.clone()])
                            .map(|(a, b)| a * b)
                            .sum::<f32>()
                            * scale
                    })
                    .collect();
                let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let exps: Vec<f32> = scores.iter().map(|score| (score - max).exp()).collect();
                let total: f32 = exps.iter().sum();
                for (weight, value) in exps.iter().zip(&values) {
                    let w = *weight / total;
                    for index in span.clone() {
                        pooled[index] += w * value[index];
                    }
                }
            }
            let projected = self.wo.forward(&pooled, false);
            let residual: Vec<f32> = token.iter().zip(&projected).map(|(a, b)| a + b).collect();
            let hidden = self.ln1.forward(&residual);
            let pre = self.ff1.forward(&hidden, false);
            let mid: Vec<f32> = pre.iter().map(|value| gelu(*value)).collect();
            let ffn = self.ff2.forward(&mid, false);
            let residual2: Vec<f32> = hidden.iter().zip(&ffn).map(|(a, b)| a + b).collect();
            out.push(self.ln2.forward(&residual2));
        }
        out
    }
}

fn bert_sequence(
    ids: &[usize],
    embeddings: &safetensors::Weights,
    position: &safetensors::Weights,
    token_type: &safetensors::Weights,
    emb_ln: &LayerNorm,
    dim: usize,
) -> Sentence {
    let mut full = Vec::with_capacity(ids.len() + 2);
    full.push(CLS_ID);
    full.extend_from_slice(ids);
    full.push(SEP_ID);
    full.iter()
        .enumerate()
        .map(|(slot, id)| {
            let mut vector = embeddings.values[id * dim..(id + 1) * dim].to_vec();
            let segment = &token_type.values[..dim];
            for (index, value) in vector.iter_mut().enumerate() {
                *value += segment[index];
            }
            if slot < position.rows {
                let offset = slot * dim;
                for (index, value) in vector.iter_mut().enumerate() {
                    *value += position.values[offset + index];
                }
            }
            emb_ln.forward(&vector)
        })
        .collect()
}

/// 解 `A w = b`（高斯-约当 + 部分主元）。
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = a.len();
    for pivot in 0..n {
        let mut best = pivot;
        for row in pivot + 1..n {
            if a[row][pivot].abs() > a[best][pivot].abs() {
                best = row;
            }
        }
        if a[best][pivot].abs() < 1e-12 {
            return None;
        }
        a.swap(pivot, best);
        b.swap(pivot, best);
        let diagonal = a[pivot][pivot];
        for value in a[pivot].iter_mut().skip(pivot) {
            *value /= diagonal;
        }
        b[pivot] /= diagonal;
        for row in 0..n {
            if row == pivot {
                continue;
            }
            let factor = a[row][pivot];
            if factor == 0.0 {
                continue;
            }
            let pivot_tail: Vec<f64> = a[pivot][pivot..].to_vec();
            for (slot, value) in a[row][pivot..].iter_mut().zip(&pivot_tail) {
                *slot -= factor * value;
            }
            b[row] -= factor * b[pivot];
        }
    }
    Some(b)
}

/// 岭回归：`(XᵗX + λI) w = Xᵗy`，特征末列补 1 作偏置。
fn ridge(features: &[Vec<f32>], target: &[f64], lambda: f64) -> Option<Vec<f64>> {
    let dim = features[0].len() + 1;
    let mut gram = vec![vec![0.0f64; dim]; dim];
    let mut cross = vec![0.0f64; dim];
    for (row, y) in features.iter().zip(target) {
        let mut values = Vec::with_capacity(dim);
        values.extend(row.iter().map(|value| f64::from(*value)));
        values.push(1.0);
        for (left, left_value) in values.iter().enumerate() {
            for right in left..dim {
                gram[left][right] += left_value * values[right];
            }
            cross[left] += left_value * y;
        }
    }
    let upper = gram.clone();
    for (left, row) in gram.iter_mut().enumerate() {
        for (right, slot) in row.iter_mut().enumerate() {
            if right < left {
                *slot = upper[right][left];
            }
            if right == left {
                *slot += lambda;
            }
        }
    }
    solve(gram, cross)
}

fn predict(features: &[Vec<f32>], weights: &[f64]) -> Vec<f64> {
    features
        .iter()
        .map(|row| {
            let mut sum = weights[weights.len() - 1];
            for (index, value) in row.iter().enumerate() {
                sum += weights[index] * f64::from(*value);
            }
            sum
        })
        .collect()
}

/// 训练折的均值与标准差。
fn stats(rows: &[Vec<f32>], dim: usize) -> (Vec<f64>, Vec<f64>) {
    let count = rows.len() as f64;
    let mut mean = vec![0.0f64; dim];
    for row in rows {
        for (slot, value) in mean.iter_mut().zip(row) {
            *slot += f64::from(*value) / count;
        }
    }
    let mut average = vec![0.0f64; dim];
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            let delta = f64::from(*value) - mean[index];
            average[index] += delta * delta / count;
        }
    }
    let std = average.iter().map(|value| value.sqrt().max(1e-8)).collect();
    (mean, std)
}

fn apply_stats(rows: &mut [Vec<f32>], mean: &[f64], std: &[f64]) {
    for row in rows.iter_mut() {
        for (index, value) in row.iter_mut().enumerate() {
            *value = ((f64::from(*value) - mean[index]) / std[index]) as f32;
        }
    }
}

/// λ × 是否标准化 的 5 折交叉验证。
fn sweep(features: &[Vec<f32>], gold: &[f64], label: &str) {
    let dim = features[0].len();
    let fold_size = features.len().div_ceil(FOLDS);
    let mut results = Vec::new();
    for &lambda in &LAMBDAS {
        for standardize in [false, true] {
            let mut folds = Vec::with_capacity(FOLDS);
            for fold in 0..FOLDS {
                let start = fold * fold_size;
                let end = ((fold + 1) * fold_size).min(features.len());
                if start >= end {
                    continue;
                }
                let mut train_x = Vec::new();
                let mut train_y = Vec::new();
                let mut test_x = Vec::new();
                let mut test_y = Vec::new();
                for index in 0..features.len() {
                    if index >= start && index < end {
                        test_x.push(features[index].clone());
                        test_y.push(gold[index]);
                    } else {
                        train_x.push(features[index].clone());
                        train_y.push(gold[index]);
                    }
                }
                if standardize {
                    let (mean, std) = stats(&train_x, dim);
                    apply_stats(&mut train_x, &mean, &std);
                    apply_stats(&mut test_x, &mean, &std);
                }
                match ridge(&train_x, &train_y, lambda) {
                    Some(weights) => {
                        let predicted = predict(&test_x, &weights);
                        folds.push(spearman(&predicted, &test_y));
                    }
                    None => folds.push(f64::NAN),
                }
            }
            let mean_fold = folds.iter().sum::<f64>() / folds.len().max(1) as f64;
            results.push(json!({
                "lambda": lambda,
                "standardize": standardize,
                "cv_mean": mean_fold,
            }));
        }
    }
    let best = results
        .iter()
        .filter_map(|value| value.get("cv_mean").and_then(serde_json::Value::as_f64))
        .fold(f64::NEG_INFINITY, f64::max);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "source": label,
            "folds": FOLDS,
            "pairs": features.len(),
            "dim": dim,
            "sweep": results,
            "best_cv": best,
        }))
        .expect("json")
    );
}

/// 合成数据自测：`y = 2*x0 - 1.5*x1 + 0.5`。
fn selftest() {
    let n = 400usize;
    let d = 8usize;
    let mut seed = 12345u64;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f32) / ((1u32 << 24) as f32) * 2.0 - 1.0
    };
    let mut x = vec![vec![0.0f32; d]; n];
    let mut y = vec![0.0f64; n];
    for row in 0..n {
        for value in x[row].iter_mut() {
            *value = rnd();
        }
        y[row] = 2.0 * f64::from(x[row][0]) - 1.5 * f64::from(x[row][1]) + 0.5;
    }
    let weights = ridge(&x, &y, 1e-6).expect("ridge solve");
    let predicted = predict(&x, &weights);
    let mean_y: f64 = y.iter().sum::<f64>() / n as f64;
    let ss_tot: f64 = y.iter().map(|v| (v - mean_y) * (v - mean_y)).sum();
    let ss_res: f64 = y
        .iter()
        .zip(&predicted)
        .map(|(a, b)| (a - b) * (a - b))
        .sum();
    println!(
        "{}",
        json!({
            "selftest": "y = 2*x0 - 1.5*x1 + 0.5",
            "r2": 1.0 - ss_res / ss_tot,
            "w0": weights[0],
            "w1": weights[1],
            "bias": weights[weights.len() - 1],
        })
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next();
    if first.as_deref() == Some("selftest") {
        selftest();
        return;
    }
    let model =
        first.expect("用法: classifier_sts_probe [selftest] <model> <vocab> <test.tsv> [对数]");
    let vocab_path = args.next().expect("vocab.txt");
    let tsv_path = args.next().expect("test.tsv");
    let pairs = read_pairs(
        &tsv_path,
        args.next()
            .map(|value| value.parse().expect("pairs"))
            .unwrap_or(1379),
    );

    // 特征缓存：提取一次落盘，之后扫超参是秒级。
    if let Ok(path) = std::env::var("STS_FEATURE_CACHE")
        && let Ok(raw) = std::fs::read(&path)
    {
        let mut cursor = 0usize;
        let rows = u32::from_le_bytes(raw[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let cols = u32::from_le_bytes(raw[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let mut features = Vec::with_capacity(rows);
        for _ in 0..rows {
            let mut row = Vec::with_capacity(cols);
            for _ in 0..cols {
                row.push(f32::from_le_bytes(
                    raw[cursor..cursor + 4].try_into().unwrap(),
                ));
                cursor += 4;
            }
            features.push(row);
        }
        let mut gold = Vec::with_capacity(rows);
        for _ in 0..rows {
            gold.push(f64::from(f32::from_le_bytes(
                raw[cursor..cursor + 4].try_into().unwrap(),
            )));
            cursor += 4;
        }
        sweep(&features, &gold, &format!("cache:{path}"));
        return;
    }

    let vocab = load_vocab(&vocab_path);
    let embeddings = safetensors::read_tensor(&model, EMBEDDINGS).expect("embeddings");
    let position = safetensors::read_tensor(&model, POSITIONS).expect("positions");
    let token_type = safetensors::read_tensor(&model, TOKEN_TYPE).expect("token type");
    let emb_ln = LayerNorm::new(
        safetensors::read_vector(&model, EMB_LN_W).expect("emb ln w"),
        safetensors::read_vector(&model, EMB_LN_B).expect("emb ln b"),
    )
    .expect("emb ln");
    let dim = embeddings.cols;
    let layers: Vec<BertLayer> = (0..6).map(|layer| BertLayer::load(&model, layer)).collect();
    let gold: Vec<f64> = pairs.iter().map(|(_, _, score)| *score).collect();

    let mut features: Vec<Vec<f32>> = Vec::with_capacity(pairs.len());
    let mut cosines: Vec<f64> = Vec::with_capacity(pairs.len());
    for (left, right, _) in &pairs {
        let mut pooled = Vec::with_capacity(2);
        for sentence in [left, right] {
            let ids = encode(sentence, &vocab);
            let mut sequence =
                bert_sequence(&ids, &embeddings, &position, &token_type, &emb_ln, dim);
            for layer in &layers {
                sequence = layer.forward(&sequence);
            }
            let count = sequence.len() as f32;
            let mut mean = vec![0.0f32; dim];
            for token in &sequence {
                for (slot, value) in mean.iter_mut().zip(token) {
                    *slot += value;
                }
            }
            for slot in mean.iter_mut() {
                *slot /= count;
            }
            pooled.push(mean);
        }
        cosines.push(cosine(&pooled[0], &pooled[1]));
        let mut feature = pooled[0].clone();
        feature.extend_from_slice(&pooled[1]);
        features.push(feature);
    }

    if let Ok(path) = std::env::var("STS_FEATURE_CACHE") {
        let mut raw: Vec<u8> = Vec::new();
        raw.extend_from_slice(&(features.len() as u32).to_le_bytes());
        raw.extend_from_slice(&((dim * 2) as u32).to_le_bytes());
        for row in &features {
            for value in row {
                raw.extend_from_slice(&value.to_le_bytes());
            }
        }
        for value in &gold {
            raw.extend_from_slice(&(*value as f32).to_le_bytes());
        }
        std::fs::write(&path, raw).expect("write cache");
        println!(
            "{}",
            json!({
                "cached": path,
                "pairs": features.len(),
                "cosine_baseline": spearman(&cosines, &gold),
            })
        );
    }

    sweep(&features, &gold, "fresh");
}
