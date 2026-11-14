//! 导出 MiniLM 第 0 层 query 段的**真实输入激活**（embedding 输出，384 维）。
//!
//! 来源：`vocab.txt` 里的真实词 → 真实 `word_embeddings`/`position`/`token_type`/`LayerNorm`
//! 前向。本机没有 STS 语料，但激活分布来自真实权重与真实词表，不是 `uniform(-1,1)`。
//!
//! 用法：`real_activations <model> <vocab.txt> [count] [--out <json>]`

use serde_json::json;
use trit_wspace::safetensors;
use trit_wspace::transformer_f32::LayerNorm;

#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use common::{encode, load_vocab, mean};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";
const CLS_ID: usize = 101;
const SEP_ID: usize = 102;

fn bert_sequence(
    ids: &[usize],
    embeddings: &safetensors::Weights,
    position: &safetensors::Weights,
    token_type: &safetensors::Weights,
    emb_ln: &LayerNorm,
    dim: usize,
) -> Vec<Vec<f32>> {
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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(
        args.len() >= 2,
        "用法: real_activations <model> <vocab> [count] [--out <json>]"
    );
    let (model, vocab_path) = (&args[0], &args[1]);
    let mut count = 256usize;
    let mut out = None;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--out" => {
                out = Some(args[index + 1].clone());
                index += 2;
            }
            other => {
                count = other.parse().expect("count");
                index += 1;
            }
        }
    }
    let vocab = load_vocab(vocab_path);
    let text = std::fs::read_to_string(vocab_path).expect("vocab");
    let words: Vec<&str> = text
        .lines()
        .filter(|word| !word.starts_with("##") && !word.starts_with('[') && word.len() > 1)
        .collect();
    let embeddings = safetensors::read_tensor(model, EMBEDDINGS).expect("embeddings");
    let position = safetensors::read_tensor(model, POSITIONS).expect("positions");
    let token_type = safetensors::read_tensor(model, TOKEN_TYPE).expect("token type");
    let emb_ln = LayerNorm::new(
        safetensors::read_vector(model, EMB_LN_W).expect("emb ln w"),
        safetensors::read_vector(model, EMB_LN_B).expect("emb ln b"),
    )
    .expect("emb ln");
    let dim = embeddings.cols;
    let mut vectors = Vec::with_capacity(count);
    for word in words.iter().take(count) {
        let ids = encode(word, &vocab);
        if ids.is_empty() {
            continue;
        }
        let sequence = bert_sequence(&ids, &embeddings, &position, &token_type, &emb_ln, dim);
        vectors.push(mean(&sequence, dim));
    }
    let report = json!({
        "source": {"model": model, "vocab": vocab_path, "words": vectors.len()},
        "cols": dim,
        "vectors": vectors,
    });
    let text = serde_json::to_string(&report).expect("json");
    match out {
        Some(path) => {
            std::fs::write(&path, text + "\n").expect("写激活失败");
            println!("写入 {path}（{} 条，{} 维）", vectors.len(), dim);
        }
        None => println!("{text}"),
    }
}
