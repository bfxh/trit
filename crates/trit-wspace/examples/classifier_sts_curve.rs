//! 固定验证批次的 loss 曲线：训练到底有没有在降低损失？
//!
//! `classifier_sts_contrastive` 报的 `loss_first` / `loss_last` 取自**不同批次**
//! （`start = step*batch % len`），批次难度不同、本就不可比——"loss 从 0.486 升到 1.80"
//! 是这个缺陷造成的假象，不是发散。
//!
//! 本示例划出一个**固定的验证小批**（不参与训练），每 `probe_every` 步在它上面测 loss，
//! 于是曲线单调可比。同时仍报 test Spearman，看指标是否跟着动。
//!
//! 用法：`classifier_sts_curve <model> <vocab> <train.tsv> <test.tsv> [步数] [lr] [tau] [probe_every]`

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
use contrastive::contrastive_grads;

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

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().expect("用法: classifier_sts_curve <model> <vocab> <train.tsv> <test.tsv> [步数] [lr] [tau] [probe_every]");
    let vocab_path = args.next().expect("vocab.txt");
    let train_tsv = args.next().expect("train.tsv");
    let test_tsv = args.next().expect("test.tsv");
    let steps: usize = args
        .next()
        .map(|v| v.parse().expect("steps"))
        .unwrap_or(100);
    let lr: f32 = args.next().map(|v| v.parse().expect("lr")).unwrap_or(1e-5);
    let tau: f32 = args.next().map(|v| v.parse().expect("tau")).unwrap_or(0.05);
    let probe_every: usize = args
        .next()
        .map(|v| v.parse().expect("probe_every"))
        .unwrap_or(20);

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
    let mut all_data: Vec<(Sentence, Sentence)> = Vec::with_capacity(train_pairs.len());
    for (left, right, _) in &train_pairs {
        all_data.push((
            freeze(encode_sentence(left)),
            freeze(encode_sentence(right)),
        ));
    }
    // 固定验证批：末尾 64 对，**不参与训练**。
    let probe_size = 64usize.min(all_data.len() / 4);
    let split = all_data.len() - probe_size;
    let (train_data, probe_data) = all_data.split_at(split);
    let train_data = train_data.to_vec();
    let probe_data = probe_data.to_vec();

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
    let probe_loss = |layer: &BertLayer| -> f32 {
        // 只要 loss：丢弃梯度（probe 只有 64 对，开销可接受）。
        contrastive_grads(layer, &probe_data, tau).0
    };

    let before = evaluate(&layer5, &test_data, &gold);
    let mut curve: Vec<(usize, f32)> = vec![(0, probe_loss(&layer5))];
    let batch = 16usize;
    for step in 1..=steps {
        let start = ((step - 1) * batch) % train_data.len();
        let slice: Vec<(Sentence, Sentence)> = (0..batch)
            .map(|offset| train_data[(start + offset) % train_data.len()].clone())
            .collect();
        let (_, grads) = contrastive_grads(&layer5, &slice, tau);
        apply_sgd(&mut layer5, &grads, lr);
        if step % probe_every == 0 {
            curve.push((step, probe_loss(&layer5)));
        }
    }
    let after = evaluate(&layer5, &test_data, &gold);

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "mode": "fixed-probe loss curve (contrastive)",
            "train_pairs": train_data.len(),
            "probe_pairs": probe_data.len(),
            "test_pairs": test_data.len(),
            "steps": steps,
            "batch": batch,
            "lr": lr,
            "tau": tau,
            "probe_every": probe_every,
            "probe_loss_curve": curve
                .iter()
                .map(|(step, loss)| json!({ "step": step, "loss": loss }))
                .collect::<Vec<_>>(),
            "spearman_before": before,
            "spearman_after": after,
        }))
        .expect("json")
    );
}
