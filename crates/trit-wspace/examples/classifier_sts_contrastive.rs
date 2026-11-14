//! 换成**对比学习目标**（in-batch negatives）训练最后一层，看 STS 能否提升。
//!
//! `classifier_sts_train` 用余弦-MSE 目标（`(cos - score/5)²`），100 步后 loss 降 4.8%
//! 但 test Spearman 一动不动（0.820376 → 0.820399）。
//!
//! 但 MiniLM 本身是用**对比学习**训出来的，余弦-MSE 可能根本不是对的路子。本示例换成
//! `L = -log softmax_j(cos(u_i, v_j)/τ)[i]`（正例是配对句，负例是同批其它句），
//! 其余设置保持可比：冻结 layer 0–4、只训 layer 5、SGD。
//!
//! 用法：`classifier_sts_contrastive <model> <vocab> <train.tsv> <test.tsv> [步数] [lr] [tau]`

use serde_json::json;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::LayerNorm;

#[path = "sts/bert.rs"]
#[allow(dead_code)]
mod bert;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;
#[path = "sts/contrastive.rs"]
#[allow(dead_code)]
mod contrastive;

use bert::{BertLayer, Grads, bert_sequence};
use common::{Sentence, cosine, encode, load_vocab, mean, read_pairs, spearman};
use contrastive::{contrastive_grads, scale_grads};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";

fn apply_sgd(layer: &mut BertLayer, grads: &Grads, lr: f32) {
    let step = |param: &mut [f32], gradient: &[f32]| {
        for (slot, value) in param.iter_mut().zip(gradient) {
            *slot -= lr * value;
        }
    };
    step(&mut layer.wq.weight, &grads.wq);
    step(&mut layer.wq.bias, &grads.bq);
    step(&mut layer.wk.weight, &grads.wk);
    step(&mut layer.wk.bias, &grads.bk);
    step(&mut layer.wv.weight, &grads.wv);
    step(&mut layer.wv.bias, &grads.bv);
    step(&mut layer.wo.weight, &grads.wo);
    step(&mut layer.wo.bias, &grads.bo);
    step(&mut layer.ln1.weight, &grads.ln1_w);
    step(&mut layer.ln1.bias, &grads.ln1_b);
    step(&mut layer.ff1.weight, &grads.ff1_w);
    step(&mut layer.ff1.bias, &grads.ff1_b);
    step(&mut layer.ff2.weight, &grads.ff2_w);
    step(&mut layer.ff2.bias, &grads.ff2_b);
    step(&mut layer.ln2.weight, &grads.ln2_w);
    step(&mut layer.ln2.bias, &grads.ln2_b);
}

/// 对比损失的梯度校验：随机小批 + 方向导数（不依赖分词与预计算，秒级）。
fn gradcheck_contrastive(model: &str) {
    let mut layer = BertLayer::load(model, 5);
    let dim = layer.wq.cols;
    let mut seed = 424242u64;
    let mut rnd = || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((seed >> 40) as f32) / ((1u32 << 24) as f32) * 2.0 - 1.0
    };
    let mut pairs: Vec<(Sentence, Sentence)> = Vec::with_capacity(4);
    for _ in 0..4 {
        let left: Sentence = (0..4)
            .map(|_| (0..dim).map(|_| rnd() * 0.3).collect())
            .collect();
        let right: Sentence = (0..4)
            .map(|_| (0..dim).map(|_| rnd() * 0.3).collect())
            .collect();
        pairs.push((left, right));
    }
    let tau = 0.05f32;
    let (loss, grads) = contrastive_grads(&layer, &pairs, tau);

    let mut direction: Vec<f32> = Vec::with_capacity(layer.wq.weight.len());
    for _ in 0..layer.wq.weight.len() {
        direction.push(rnd());
    }
    let analytic: f32 = grads.wq.iter().zip(&direction).map(|(g, v)| g * v).sum();
    let eps = 1e-4f32;
    let original = layer.wq.weight.clone();
    for (index, delta) in direction.iter().enumerate() {
        layer.wq.weight[index] = original[index] + eps * delta;
    }
    let (plus, _) = contrastive_grads(&layer, &pairs, tau);
    for (index, delta) in direction.iter().enumerate() {
        layer.wq.weight[index] = original[index] - eps * delta;
    }
    let (minus, _) = contrastive_grads(&layer, &pairs, tau);
    layer.wq.weight = original;
    let numeric = (plus - minus) / (2.0 * eps);
    let denominator = analytic.abs().max(numeric.abs()).max(1e-9);
    println!(
        "{}",
        json!({
            "mode": "contrastive gradcheck",
            "loss": loss,
            "direction_analytic": analytic,
            "direction_numeric": numeric,
            "relative_error": (analytic - numeric).abs() / denominator,
        })
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args.next().expect(
        "用法: classifier_sts_contrastive [gradcheck] <model> <vocab> <train.tsv> <test.tsv> [步数] [lr] [tau]",
    );
    if first == "gradcheck" {
        let real = args.next().expect("model");
        gradcheck_contrastive(&real);
        return;
    }
    let model = first;
    let vocab_path = args.next().expect("vocab.txt");
    let train_tsv = args.next().expect("train.tsv");
    let test_tsv = args.next().expect("test.tsv");
    let steps: usize = args
        .next()
        .map(|value| value.parse().expect("steps"))
        .unwrap_or(100);
    let lr: f32 = args
        .next()
        .map(|value| value.parse().expect("lr"))
        .unwrap_or(1e-5);
    let tau: f32 = args
        .next()
        .map(|value| value.parse().expect("tau"))
        .unwrap_or(0.05);

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
    let frozen: Vec<BertLayer> = (0..5).map(|layer| BertLayer::load(&model, layer)).collect();
    let mut layer5 = BertLayer::load(&model, 5);

    let encode_sentence = |text: &str| -> Sentence {
        let ids = encode(text, &vocab);
        bert_sequence(&ids, &embeddings, &position, &token_type, &emb_ln, dim)
    };
    let freeze = |sequence: Sentence| -> Sentence {
        let mut current = sequence;
        for layer in &frozen {
            current = layer.forward_cached(&current).0;
        }
        current
    };

    let train_pairs = read_pairs(&train_tsv, 800);
    let test_pairs = read_pairs(&test_tsv, 1379);
    let mut train_data: Vec<(Sentence, Sentence)> = Vec::with_capacity(train_pairs.len());
    for (left, right, _) in &train_pairs {
        train_data.push((
            freeze(encode_sentence(left)),
            freeze(encode_sentence(right)),
        ));
    }
    let mut test_data: Vec<(Sentence, Sentence)> = Vec::with_capacity(test_pairs.len());
    let gold: Vec<f64> = test_pairs.iter().map(|(_, _, score)| *score).collect();
    for (left, right, _) in &test_pairs {
        test_data.push((
            freeze(encode_sentence(left)),
            freeze(encode_sentence(right)),
        ));
    }

    let evaluate = |layer: &BertLayer, data: &[(Sentence, Sentence)], gold: &[f64]| -> f64 {
        let cosines: Vec<f64> = data
            .iter()
            .map(|(left, right)| {
                let u = mean(&layer.forward_cached(left).0, dim);
                let v = mean(&layer.forward_cached(right).0, dim);
                cosine(&u, &v)
            })
            .collect();
        spearman(&cosines, gold)
    };

    let before = evaluate(&layer5, &test_data, &gold);
    let batch = 16usize;
    let mut losses = Vec::with_capacity(steps);
    for step in 0..steps {
        let start = (step * batch) % train_data.len();
        let slice: Vec<(Sentence, Sentence)> = (0..batch)
            .map(|offset| train_data[(start + offset) % train_data.len()].clone())
            .collect();
        let (loss, mut grads) = contrastive_grads(&layer5, &slice, tau);
        scale_grads(&mut grads, 1.0);
        apply_sgd(&mut layer5, &grads, lr);
        losses.push(loss);
    }
    let after = evaluate(&layer5, &test_data, &gold);

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "contrastive (in-batch negatives)",
            "train_pairs": train_data.len(),
            "test_pairs": test_data.len(),
            "steps": steps,
            "batch": batch,
            "lr": lr,
            "tau": tau,
            "loss_first": losses.first().copied().unwrap_or(0.0),
            "loss_last": losses.last().copied().unwrap_or(0.0),
            "spearman_before": before,
            "spearman_after": after,
        }))
        .expect("json")
    );
}
