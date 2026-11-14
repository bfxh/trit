//! STS 代理任务：判据是任务排序正确（Spearman），不是复现某个网络。

use serde_json::json;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Net, Trit};

mod proj;

use proj::*;

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

struct Args {
    safetensors: String,
    query: String,
    embeddings: String,
    samples: usize,
    pca_samples: usize,
    max_k: usize,
    iterations: usize,
    threshold: f32,
    seed: u64,
    dims: Vec<usize>,
    out: Option<String>,
}

fn parse_list(value: &str) -> Vec<usize> {
    value
        .split(',')
        .map(|p| p.trim().parse().expect("列表项"))
        .collect()
}

fn parse_args() -> Args {
    let mut args = Args {
        safetensors: String::new(),
        query: QUERY.to_string(),
        embeddings: EMBEDDINGS.to_string(),
        samples: 2048,
        pca_samples: 2048,
        max_k: 128,
        iterations: 12,
        threshold: 0.5,
        seed: 7,
        dims: vec![1, 2, 4, 8, 16, 32, 64, 128, 384],
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--safetensors" => args.safetensors = next(),
            "--query" => args.query = next(),
            "--embeddings" => args.embeddings = next(),
            "--samples" => args.samples = next().parse().expect("samples"),
            "--pca-samples" => args.pca_samples = next().parse().expect("pca-samples"),
            "--max-k" => args.max_k = next().parse().expect("max-k"),
            "--iterations" => args.iterations = next().parse().expect("iterations"),
            "--threshold" => args.threshold = next().parse().expect("threshold"),
            "--seed" => args.seed = next().parse().expect("seed"),
            "--dims" => args.dims = parse_list(&next()),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.safetensors.is_empty(), "需要 --safetensors");
    args
}

fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize row");
    (0..values.len())
        .map(|col| layer.trit(0, col).expect("trit"))
        .collect()
}

fn cos_f32(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let norm = left.iter().map(|v| v * v).sum::<f32>().sqrt()
        * right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 { 0.0 } else { dot / norm }
}

fn cos_trit(left: &[Trit], right: &[Trit]) -> f32 {
    let a: Vec<f32> = left.iter().map(|t| f32::from(t.to_i8())).collect();
    let b: Vec<f32> = right.iter().map(|t| f32::from(t.to_i8())).collect();
    cos_f32(&a, &b)
}

fn ranks(values: &[f32]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..values.len()).collect();
    idx.sort_by(|a, b| {
        values[*a]
            .partial_cmp(&values[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut out = vec![0.0f64; values.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && (values[idx[j + 1]] - values[idx[i]]).abs() < 1e-12 {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for t in i..=j {
            out[idx[t]] = avg;
        }
        i = j + 1;
    }
    out
}

fn spearman(left: &[f32], right: &[f32]) -> f64 {
    let (ra, rb) = (ranks(left), ranks(right));
    let n = ra.len() as f64;
    let m = (n + 1.0) / 2.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in ra.iter().zip(&rb) {
        num += (x - m) * (y - m);
        da += (x - m) * (x - m);
        db += (y - m) * (y - m);
    }
    if da == 0.0 || db == 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

fn reduced_single(query: &Weights, dim: usize, basis: &[Vec<f64>]) -> Net {
    let k = basis.len();
    let base = query.to_layer().expect("ternary query");
    let bias: Vec<i8> = base.bias().iter().map(|t| t.to_i8()).collect();
    let mut trits = vec![0i8; query.rows * k];
    for row in 0..query.rows {
        let w = &query.values[row * dim..(row + 1) * dim];
        for (i, u) in basis.iter().enumerate() {
            let acc: f64 = u.iter().zip(w).map(|(a, b)| a * f64::from(*b)).sum();
            trits[row * k + i] = (acc > 0.0) as i8 - (acc < 0.0) as i8;
        }
    }
    Net::new(vec![
        Layer::new(query.rows, k, &trits, &bias).expect("reduced layer"),
    ])
    .expect("net")
}

fn main() {
    let args = parse_args();
    let query = safetensors::read_tensor(&args.safetensors, &args.query).expect("query tensor");
    let embeddings =
        safetensors::read_tensor(&args.safetensors, &args.embeddings).expect("embeddings tensor");
    assert_eq!(query.cols, embeddings.cols, "query 列数必须等于词向量维度");
    let dim = query.cols;

    let mut rng = Rng::new(args.seed);
    let mut cands: Vec<(Vec<f32>, Vec<f32>, f32)> = Vec::with_capacity(args.samples * 2);
    while cands.len() < args.samples * 2 {
        let i = (rng.next() % embeddings.rows as u64) as usize;
        let j = (rng.next() % embeddings.rows as u64) as usize;
        if i == j {
            continue;
        }
        let a = embeddings.values[i * dim..(i + 1) * dim].to_vec();
        let b = embeddings.values[j * dim..(j + 1) * dim].to_vec();
        let score = cos_f32(&a, &b);
        cands.push((a, b, score));
    }
    cands.sort_by(|x, y| x.2.partial_cmp(&y.2).unwrap_or(std::cmp::Ordering::Equal));
    let quarter = cands.len() / 4;
    if quarter == 0 {
        panic!("候选太少，无法切分位数");
    }
    let mut pairs: Vec<(Vec<f32>, Vec<f32>, bool)> = Vec::with_capacity(quarter * 2);
    for c in &cands[cands.len() - quarter..] {
        pairs.push((c.0.clone(), c.1.clone(), true));
    }
    for c in &cands[..quarter] {
        pairs.push((c.0.clone(), c.1.clone(), false));
    }
    for i in (1..pairs.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        pairs.swap(i, j);
    }
    let positives = quarter;
    let pos_cut = cands[cands.len() - quarter].2;
    let neg_cut = cands[quarter - 1].2;

    let moment = second_moment(&embeddings, dim, args.pca_samples, args.seed);
    let comps = top_components(&moment, dim, args.max_k.min(dim), args.iterations);

    let mut results = Vec::new();
    for &k in &args.dims {
        let basis = if k >= dim {
            identity(dim)
        } else if k <= comps.len() {
            comps[..k].to_vec()
        } else {
            continue;
        };
        let net = reduced_single(&query, dim, &basis);
        let mut trit_scores: Vec<f32> = Vec::with_capacity(pairs.len());
        let mut teacher_scores: Vec<f32> = Vec::with_capacity(pairs.len());
        for (a, b, _) in &pairs {
            let ga = net
                .forward(&trit_row(&project(a, &basis)))
                .expect("forward a");
            let gb = net
                .forward(&trit_row(&project(b, &basis)))
                .expect("forward b");
            trit_scores.push(cos_trit(&ga, &gb));
            teacher_scores.push(cos_f32(a, b));
        }
        results.push(json!({
            "k": k,
            "spearman": spearman(&trit_scores, &teacher_scores),
            "trits": net.trit_count(),
        }));
    }

    let report = json!({
        "task": "sts-proxy",
        "safetensors": args.safetensors,
        "threshold": args.threshold,
        "pairs": pairs.len(),
        "positive_rate": positives as f64 / pairs.len() as f64,
        "pos_cut_cos": pos_cut,
        "neg_cut_cos": neg_cut,
        "baseline_random": 0.5,
        "curves": results,
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
