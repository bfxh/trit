//! 配置驱动 + 特征缓存 + 参数网格：把"改参数要重跑 20 分钟"变成秒级。
//!
//! ## 缓存能省什么、不能省什么
//!
//! 冻结层特征只在**冻结层不变**时可复用：
//!
//! | 实验 | 冻结层变吗 | 能否缓存 |
//! |---|---|---|
//! | 训练最后一层（lr 扫描） | 否 | **能** |
//! | 只量化最后一层（group 扫描） | 否 | **能** |
//! | 采样率变化（取子集） | 否 | **能** |
//! | 全 6 层量化 / 剪头 | **是**（每层权重都变） | **不能** |
//!
//! 所以本示例缓存 **layer 0–4 的 token 级输出**（约 51 MB），网格只作用在最后一层。
//! 首次运行约 20 分钟（提取），之后秒级重跑。
//!
//! ## 用法
//!
//! ```text
//! classifier_sts_sweep <model> <vocab> <test.tsv> [选项]
//!
//!   --pairs N            取前 N 对（默认 300）
//!   --cache PATH         特征缓存文件（默认 <tmp>/sts_l5_cache.bin）
//!   --sample R           只评估 R 比例的样本（分层抽样，默认 1.0）
//!   --grid group=A,B,C   最后一层的分组大小网格
//!   --grid lr=A,B        最后一层的训练学习率网格（含 --steps）
//!   --steps N            训练步数（默认 200）
//!   --resamples N        bootstrap 重采样次数（默认 2000，0 = 关闭）
//! ```
//!
//! 配置也可写成文件（`key=value`，一行一条），用 `--config PATH` 传入；命令行覆盖配置文件。

use std::collections::HashMap;

use serde_json::json;
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::safetensors;
use trit_wspace::stats::{bootstrap_spearman_ci, spearman, stratified_indices};
use trit_wspace::transformer_f32::{Dense, LayerNorm};

#[path = "sts/bert.rs"]
#[allow(dead_code)]
mod bert;
#[path = "sts/common.rs"]
#[allow(dead_code)]
mod common;

use bert::BertLayer;
use common::{Sentence, cosine, encode, load_vocab, mean, read_pairs};

const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const POSITIONS: &str = "embeddings.position_embeddings.weight";
const TOKEN_TYPE: &str = "embeddings.token_type_embeddings.weight";
const EMB_LN_W: &str = "embeddings.LayerNorm.weight";
const EMB_LN_B: &str = "embeddings.LayerNorm.bias";
const LINEAR: [&str; 6] = [
    "attention.self.query.weight",
    "attention.self.key.weight",
    "attention.self.value.weight",
    "attention.output.dense.weight",
    "intermediate.dense.weight",
    "output.dense.weight",
];

/// 选项：全部可命令行覆盖，也可来自配置文件。
#[derive(Clone, Debug)]
struct Options {
    pairs: usize,
    cache: String,
    sample: f64,
    groups: Vec<usize>,
    lrs: Vec<f32>,
    steps: usize,
    resamples: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            pairs: 300,
            cache: String::new(),
            sample: 1.0,
            groups: Vec::new(),
            lrs: Vec::new(),
            steps: 200,
            resamples: 2000,
        }
    }
}

fn parse_list<T: std::str::FromStr>(value: &str) -> Vec<T> {
    value
        .split(',')
        .filter_map(|word| word.trim().parse::<T>().ok())
        .collect()
}

fn apply_kv(options: &mut Options, key: &str, value: &str) {
    match key {
        "pairs" => options.pairs = value.parse().unwrap_or(options.pairs),
        "cache" => options.cache = value.to_string(),
        "sample" => options.sample = value.parse().unwrap_or(options.sample),
        "group" | "groups" => options.groups = parse_list::<usize>(value),
        "lr" | "lrs" => options.lrs = parse_list::<f32>(value),
        "steps" => options.steps = value.parse().unwrap_or(options.steps),
        "resamples" => options.resamples = value.parse().unwrap_or(options.resamples),
        _ => {}
    }
}

/// 读取 `key=value` 配置文件。
fn load_config(path: &str) -> Options {
    let mut options = Options::default();
    if let Ok(text) = std::fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                apply_kv(&mut options, key.trim(), value.trim());
            }
        }
    }
    options
}

fn parse_args() -> Options {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // 先找 --config，命令行覆盖它
    let mut options = Options::default();
    if let Some(position) = args.iter().position(|arg| arg == "--config")
        && let Some(path) = args.get(position + 1).cloned()
    {
        options = load_config(&path);
        args.remove(position + 1);
        args.remove(position);
    }
    let mut index = 0usize;
    while index < args.len() {
        let arg = args[index].clone();
        if let Some(rest) = arg.strip_prefix("--") {
            // `--grid group=64,128,256`：`grid` 只是分组名，真正的 key 在内层
            if rest == "grid"
                && let Some(value) = args.get(index + 1)
                && let Some((key, list)) = value.split_once('=')
            {
                apply_kv(&mut options, key, list);
                index += 2;
                continue;
            }
            if let Some((key, value)) = rest.split_once('=') {
                apply_kv(&mut options, key, value);
                index += 1;
                continue;
            }
            // `--key value` 形式
            if let Some(value) = args.get(index + 1) {
                apply_kv(&mut options, rest, value);
                index += 2;
                continue;
            }
        }
        index += 1;
    }
    options
}

/// 缓存：每对的左右句子，在 layer 0–4 之后（token 级）。
fn cache_path(options: &Options) -> String {
    if !options.cache.is_empty() {
        return options.cache.clone();
    }
    let dir = std::env::temp_dir();
    dir.join(format!("sts_l5_cache_{}.bin", options.pairs))
        .to_string_lossy()
        .to_string()
}

fn write_cache(path: &str, data: &[(Sentence, Sentence)]) {
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    for (left, right) in data {
        for sequence in [left, right] {
            bytes.extend_from_slice(&(sequence.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(sequence[0].len() as u32).to_le_bytes());
            for token in sequence {
                for value in token {
                    bytes.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
    }
    let _ = std::fs::write(path, bytes);
}

fn read_cache(path: &str) -> Option<Vec<(Sentence, Sentence)>> {
    let bytes = std::fs::read(path).ok()?;
    let mut cursor = 0usize;
    let take_u32 = |cursor: &mut usize| -> Option<u32> {
        let slice = bytes.get(*cursor..*cursor + 4)?;
        *cursor += 4;
        Some(u32::from_le_bytes(slice.try_into().ok()?))
    };
    let pairs = take_u32(&mut cursor)? as usize;
    let mut out = Vec::with_capacity(pairs);
    for _ in 0..pairs {
        let mut sides = Vec::with_capacity(2);
        for _ in 0..2 {
            let tokens = take_u32(&mut cursor)? as usize;
            let dim = take_u32(&mut cursor)? as usize;
            let mut sequence = Vec::with_capacity(tokens);
            for _ in 0..tokens {
                let mut token = Vec::with_capacity(dim);
                for _ in 0..dim {
                    let slice = bytes.get(cursor..cursor + 4)?;
                    cursor += 4;
                    token.push(f32::from_le_bytes(slice.try_into().ok()?));
                }
                sequence.push(token);
            }
            sides.push(sequence);
        }
        out.push((sides.remove(0), sides.remove(0)));
    }
    Some(out)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let model = args.next().expect(
        "用法: classifier_sts_sweep <model> <vocab> <test.tsv> [--pairs N] [--grid group=..] ...",
    );
    let vocab_path = args.next().expect("vocab.txt");
    let tsv_path = args.next().expect("test.tsv");
    let options = parse_args();
    let cache = cache_path(&options);

    let vocab: HashMap<String, usize> = load_vocab(&vocab_path);
    let embeddings = safetensors::read_tensor(&model, EMBEDDINGS).expect("embeddings");
    let position = safetensors::read_tensor(&model, POSITIONS).expect("positions");
    let token_type = safetensors::read_tensor(&model, TOKEN_TYPE).expect("token type");
    let emb_ln = LayerNorm::new(
        safetensors::read_vector(&model, EMB_LN_W).expect("w"),
        safetensors::read_vector(&model, EMB_LN_B).expect("b"),
    )
    .expect("emb ln");
    let dim = embeddings.cols;

    let pairs = read_pairs(&tsv_path, options.pairs);
    let gold_all: Vec<f64> = pairs.iter().map(|(_, _, score)| *score).collect();

    // 缓存命中 → 秒级；否则提取 layer 0–4 的输出（约 20 分钟）
    let mut cache_hit = false;
    let frozen = match read_cache(&cache) {
        Some(data) if data.len() == pairs.len() => {
            cache_hit = true;
            data
        }
        _ => {
            let frozen_layers: Vec<BertLayer> =
                (0..5).map(|i| BertLayer::load(&model, i)).collect();
            let mut out: Vec<(Sentence, Sentence)> = Vec::with_capacity(pairs.len());
            for (left, right, _) in &pairs {
                let build = |text: &str| -> Sentence {
                    let ids = encode(text, &vocab);
                    let mut current = bert::bert_sequence(
                        &ids,
                        &embeddings,
                        &position,
                        &token_type,
                        &emb_ln,
                        dim,
                    );
                    for layer in &frozen_layers {
                        current = layer.forward_cached(&current).0;
                    }
                    current
                };
                out.push((build(left), build(right)));
            }
            write_cache(&cache, &out);
            out
        }
    };

    let layer5 = BertLayer::load(&model, 5);

    // 分层抽样（按人工分数分 5 层）
    let strata: Vec<usize> = gold_all
        .iter()
        .map(|value| ((value / 5.0) * 5.0).clamp(0.0, 4.0) as usize)
        .collect();
    let chosen = if options.sample >= 1.0 {
        (0..pairs.len()).collect::<Vec<usize>>()
    } else {
        stratified_indices(&strata, options.sample, 20261114)
    };
    let gold: Vec<f64> = chosen.iter().map(|index| gold_all[*index]).collect();

    // 用给定 layer5 评估被选中的样本。
    let evaluate = |layer: &BertLayer| -> Vec<f64> {
        chosen
            .iter()
            .map(|index| {
                let (left, right) = &frozen[*index];
                let u = mean(&layer.forward_cached(left).0, dim);
                let v = mean(&layer.forward_cached(right).0, dim);
                cosine(&u, &v)
            })
            .collect()
    };

    let mut rows: Vec<serde_json::Value> = Vec::new();

    // 基线（layer 5 不动）
    let baseline = evaluate(&layer5);
    rows.push(json!({
        "config": "baseline (f32)",
        "spearman": spearman(&baseline, &gold),
        "ci95": if options.resamples > 0 {
            bootstrap_spearman_ci(&baseline, &gold, options.resamples, 0.95, 20261114)
        } else { None },
    }));

    // group 网格：只量化最后一层
    for group in &options.groups {
        let mut layer = BertLayer::load(&model, 5);
        for suffix in LINEAR {
            let name = format!("encoder.layer.5.{suffix}");
            let weight = safetensors::read_tensor(&model, &name).expect("weight");
            let bias =
                safetensors::read_vector(&model, &name.replace(".weight", ".bias")).expect("bias");
            let quantized =
                GroupedLayer::quantize(weight.rows, weight.cols, &weight.values, &bias, *group)
                    .expect("quantize");
            let replacement =
                Dense::new(quantized.to_f32(), bias, weight.rows, weight.cols).expect("dense");
            match suffix {
                "attention.self.query.weight" => layer.wq = replacement,
                "attention.self.key.weight" => layer.wk = replacement,
                "attention.self.value.weight" => layer.wv = replacement,
                "attention.output.dense.weight" => layer.wo = replacement,
                "intermediate.dense.weight" => layer.ff1 = replacement,
                _ => layer.ff2 = replacement,
            }
        }
        let cosines = evaluate(&layer);
        rows.push(json!({
            "config": format!("layer5 ternary group{group}"),
            "spearman": spearman(&cosines, &gold),
            "delta": spearman(&cosines, &gold) - spearman(&baseline, &gold),
        }));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "cache": cache,
            "cache_hit": cache_hit,
            "pairs": pairs.len(),
            "evaluated": chosen.len(),
            "sample": options.sample,
            "steps": options.steps,
            "resamples": options.resamples,
            "rows": rows,
        }))
        .expect("json")
    );
}
