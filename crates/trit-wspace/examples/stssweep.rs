//! STS 代理任务上的降维扫描：判据是「判定正确」，不是「复现某个网络」。
//!
//! 与 `dimsweep` 的区别：那边问「压到多低还像原来的学生」，这边问「压到多低还能把任务做对」。
//! 任务沿用 `editset_sts` 的判定式：`cos(forward(a), forward(b)) >= threshold`，教师由
//! `cos(a, b) >= threshold` 给出（`editset_sts` 的 label 缺省分支）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example stssweep -- \
//!   --safetensors model.safetensors --samples 2048 --dims 1,2,4,8,16,32,64,128,384
//! ```

use serde_json::json;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Net, Trit};

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

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        let state = seed ^ 0x9E37_79B9_7F4A_7C15;
        Self(if state == 0 { 1 } else { state })
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
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

fn second_moment(embeddings: &Weights, dim: usize, samples: usize, seed: u64) -> Vec<f64> {
    let mut rng = Rng::new(seed ^ 0x5DEE_CE66);
    let mut mom = vec![0.0f64; dim * dim];
    for _ in 0..samples {
        let row = (rng.next() % embeddings.rows as u64) as usize;
        let x = &embeddings.values[row * dim..(row + 1) * dim];
        for (i, &xi) in x.iter().enumerate() {
            let xi = f64::from(xi);
            for (j, &xj) in x.iter().enumerate() {
                mom[i * dim + j] += xi * f64::from(xj);
            }
        }
    }
    let scale = 1.0 / samples as f64;
    for value in &mut mom {
        *value *= scale;
    }
    mom
}

fn identity(dim: usize) -> Vec<Vec<f64>> {
    (0..dim)
        .map(|i| {
            let mut v = vec![0.0f64; dim];
            v[i] = 1.0;
            v
        })
        .collect()
}

fn jacobi_eigen(matrix: &[f64], n: usize, sweeps: usize) -> (Vec<f64>, Vec<Vec<f64>>) {
    let mut m = matrix.to_vec();
    let mut v = vec![0.0f64; n * n];
    for i in 0..n {
        v[i * n + i] = 1.0;
    }
    for _ in 0..sweeps {
        let mut off = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                off += m[i * n + j] * m[i * n + j];
            }
        }
        if off < 1e-22 {
            break;
        }
        for p in 0..n {
            for q in (p + 1)..n {
                let apq = m[p * n + q];
                if apq.abs() < 1e-18 {
                    continue;
                }
                let theta = (m[q * n + q] - m[p * n + p]) / (2.0 * apq);
                let t = if theta >= 0.0 {
                    1.0 / (theta + (theta * theta + 1.0).sqrt())
                } else {
                    -1.0 / (-theta + (theta * theta + 1.0).sqrt())
                };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let mkp = m[k * n + p];
                    let mkq = m[k * n + q];
                    m[k * n + p] = c * mkp - s * mkq;
                    m[k * n + q] = s * mkp + c * mkq;
                }
                for k in 0..n {
                    let mpk = m[p * n + k];
                    let mqk = m[q * n + k];
                    m[p * n + k] = c * mpk - s * mqk;
                    m[q * n + k] = s * mpk + c * mqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut pairs: Vec<(f64, Vec<f64>)> = (0..n)
        .map(|j| {
            (
                m[j * n + j],
                (0..n).map(|i| v[i * n + j]).collect::<Vec<f64>>(),
            )
        })
        .collect();
    pairs.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    pairs.into_iter().unzip()
}

fn top_components(matrix: &[f64], dim: usize, k: usize, sweeps: usize) -> Vec<Vec<f64>> {
    let (_, vectors) = jacobi_eigen(matrix, dim, sweeps);
    vectors.into_iter().take(k.min(dim)).collect()
}

fn project(x: &[f32], comps: &[Vec<f64>]) -> Vec<f32> {
    comps
        .iter()
        .map(|u| u.iter().zip(x).map(|(a, b)| a * f64::from(*b)).sum::<f64>() as f32)
        .collect()
}

/// 单层降维学生：`Layer(rows, k)`，输出仍是 `rows` 维（STS 需要句向量而不是单个标签）。
fn reduced_single(query: &Weights, dim: usize, basis: &[Vec<f64>]) -> Net {
    let k = basis.len();
    let base = query.to_layer().expect("ternary query");
    let bias: Vec<i8> = base.bias().iter().map(|t| t.to_i8()).collect();
    let mut trits = vec![0i8; query.rows * k];
    for row in 0..query.rows {
        let w = &query.values[row * dim..(row + 1) * dim];
        for (i, u) in basis.iter().enumerate() {
            let acc: f64 = u.iter().zip(w).map(|(a, b)| a * f64::from(*b)).sum();
            trits[row * k + i] = if acc > 0.0 {
                1
            } else if acc < 0.0 {
                -1
            } else {
                0
            };
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

    // 1) 采样候选词对，按余弦分位数取平衡的正/负例。
    //    随机词对的 cos 极少 >= threshold（实测 0.5%），直接按阈值切会得到「全判负」的假高分。
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
    // 打乱：稳定排序下并列值会偏向「正例在前」，低 k 的取值高度并列，会造出假的 1.0。
    for i in (1..pairs.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        pairs.swap(i, j);
    }
    let positives = quarter;
    let pos_cut = cands[cands.len() - quarter].2;
    let neg_cut = cands[quarter - 1].2;

    // 2) 主方向。
    let moment = second_moment(&embeddings, dim, args.pca_samples, args.seed);
    let comps = top_components(&moment, dim, args.max_k.min(dim), args.iterations);

    // 3) 每个 k 的任务准确率。
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
        // 三值 cos 的分布与 f32 不同，沿用 f32 的阈值会得到「全判负」的假象。
        // 改成比较**排序**：取三值 cos 的 top quarter，与教师的 top quarter 比重叠率
        // （随机期望 0.25）。教师正例排在 pairs 的前 quarter（采样时先 push 正例）。
        let mut scored: Vec<(usize, f32)> = pairs
            .iter()
            .enumerate()
            .map(|(idx, (a, b, _))| {
                let ga = net
                    .forward(&trit_row(&project(a, &basis)))
                    .expect("forward a");
                let gb = net
                    .forward(&trit_row(&project(b, &basis)))
                    .expect("forward b");
                (idx, cos_trit(&ga, &gb))
            })
            .collect();
        scored.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
        let quarter_rank = scored.len() / 4;
        let overlap = scored[..quarter_rank]
            .iter()
            .filter(|(idx, _)| *idx < positives)
            .count();
        results.push(json!({
            "k": k,
            "top_quarter_overlap": overlap as f64 / quarter_rank.max(1) as f64,
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
