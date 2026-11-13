//! 降维运行扫描（dimsweep）：一段「思考」要多少维度、多少步才够。
//!
//! 三条曲线，同一个协议（真实 `word_embeddings` 行 + f32 教师 + edit/keep 双集合）：
//!
//! 1. **宽度**：把 384 维输入投影到前 `k` 个主方向（真实词向量的二阶矩），降维学生是
//!    `Layer::new(rows, k)` + 原读出层，报 edit/keep 两个集合的 loss 随 `k` 怎么变。
//! 2. **步数**：在 `k` 维学生上跑 `repair`（带 `keep` 约束），报预算
//!    `(max_edits, beam_width)` 与 `evaluated`，找「再走也没用」的饱和点。
//! 3. **必要性**：在最大子空间上逐维剔除，报每一维的边际违规增量，排序出哪几维承载判据。
//!
//! 输出 JSON：三条曲线 + `minimum`（使 edit_loss=0 且 keep_loss=0 的最小 (k, max_edits, beam_width)）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example dimsweep -- \
//!   --safetensors model.safetensors --samples 512 --edit-limit 8 --keep-limit 64
//! ```
//!
//! 与 `editset_gate` 的区别：那边问「补丁能不能同时过 edit/keep」，这边问
//! **「把思考降到多低还够用」**——宽度、步数、维度必要性一起量。

use serde_json::{Value, json};
use trit_wspace::repair::{RepairOptions, repair};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Net, Oracle, Trit};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

struct Args {
    safetensors: String,
    query: String,
    embeddings: String,
    readout_row: usize,
    samples: usize,
    edit_limit: usize,
    keep_limit: usize,
    seed: u64,
    pca_samples: usize,
    max_k: usize,
    iterations: usize,
    dims: Vec<usize>,
    steps_dims: Vec<usize>,
    max_edits: Vec<usize>,
    beam_widths: Vec<usize>,
    out: Option<String>,
}

fn parse_list(value: &str) -> Vec<usize> {
    value
        .split(',')
        .map(|part| part.trim().parse().expect("列表项"))
        .collect()
}

fn parse_args() -> Args {
    let mut args = Args {
        safetensors: String::new(),
        query: QUERY.to_string(),
        embeddings: EMBEDDINGS.to_string(),
        readout_row: 0,
        samples: 512,
        edit_limit: 8,
        keep_limit: 64,
        seed: 7,
        pca_samples: 2048,
        max_k: 128,
        iterations: 12,
        dims: vec![384, 128, 64, 32, 16],
        steps_dims: vec![32, 16],
        max_edits: vec![1, 2],
        beam_widths: vec![8],
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--safetensors" => args.safetensors = next(),
            "--query" => args.query = next(),
            "--embeddings" => args.embeddings = next(),
            "--readout-row" => args.readout_row = next().parse().expect("readout-row"),
            "--samples" => args.samples = next().parse().expect("samples"),
            "--edit-limit" => args.edit_limit = next().parse().expect("edit-limit"),
            "--keep-limit" => args.keep_limit = next().parse().expect("keep-limit"),
            "--seed" => args.seed = next().parse().expect("seed"),
            "--pca-samples" => args.pca_samples = next().parse().expect("pca-samples"),
            "--max-k" => args.max_k = next().parse().expect("max-k"),
            "--iterations" => args.iterations = next().parse().expect("iterations"),
            "--dims" => args.dims = parse_list(&next()),
            "--steps-dims" => args.steps_dims = parse_list(&next()),
            "--max-edits" => args.max_edits = parse_list(&next()),
            "--beam-widths" => args.beam_widths = parse_list(&next()),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.safetensors.is_empty(), "需要 --safetensors");
    assert!(args.max_k > 0, "--max-k 必须 > 0");
    args
}

/// 确定性 xorshift64*。
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

fn sign_of(score: f32) -> Trit {
    if score > 0.0 {
        Trit::Pos
    } else if score < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

/// 一行 f32 按 absmean 三值化（与 `Weights::to_layer` 同一实现）。
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

fn trit_row_i8(values: &[f32]) -> Vec<i8> {
    trit_row(values).iter().map(|t| t.to_i8()).collect()
}

/// 读出层：1×dim 的三值线性层。
fn readout_layer(readout: &[f32]) -> Layer {
    Layer::new(1, readout.len(), &trit_row_i8(readout), &[0]).expect("readout layer")
}

/// f32 教师前向：`query × x`。
fn teacher_forward(query: &Weights, x: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; query.rows];
    for (row, slot) in out.iter_mut().enumerate() {
        let weights = &query.values[row * query.cols..(row + 1) * query.cols];
        *slot = weights
            .iter()
            .zip(x)
            .map(|(weight, value)| weight * value)
            .sum();
    }
    out
}

fn dot(readout: &[f32], values: &[f32]) -> f32 {
    readout.iter().zip(values).map(|(r, v)| r * v).sum()
}

/// 一个样本：原始 384 维输入 + 全维 f32 教师的标签。
#[derive(Clone)]
struct Sample {
    x: Vec<f32>,
    teacher: Trit,
}

fn full_student(query: &Weights, readout: &[f32]) -> Net {
    Net::new(vec![
        query.to_layer().expect("ternary query"),
        readout_layer(readout),
    ])
    .expect("student net")
}

fn sample_dataset(
    query: &Weights,
    embeddings: &Weights,
    readout: &[f32],
    args: &Args,
) -> Vec<Sample> {
    let dim = query.cols;
    let mut rng = Rng::new(args.seed);
    let budget = args.samples.max(args.edit_limit + args.keep_limit);
    let mut out = Vec::with_capacity(budget);
    while out.len() < budget {
        let row = (rng.next() % embeddings.rows as u64) as usize;
        let x = embeddings.values[row * dim..(row + 1) * dim].to_vec();
        let teacher = sign_of(dot(readout, &teacher_forward(query, &x)));
        if teacher == Trit::Zero {
            continue;
        }
        out.push(Sample { x, teacher });
    }
    out
}

/// 真实词向量行的二阶矩 `M = E[x x^T]`（dim×dim，行主序 f64）。
///
/// 不去均值：`absmean` 三值化本来就不中心化，且这样降维层不需要常数偏置项
/// （`w·x ≈ (U^T w)·(U^T x)`），三值偏置不会吃掉一个 f32 常数。
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

/// Jacobi 特征分解（对称矩阵）：返回按特征值降序的 (特征值, 方向)。
///
/// 384x384 的完整分解在这个规模上很快，且基严格正交：不会出现「k 增大反而更差」
/// 这种非单调伪影（幂迭代 + 一次 Gram-Schmidt 在谱比接近 1 时做不到）。
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

/// 前 `k` 个主方向（特征值降序）。
fn top_components(
    matrix: &[f64],
    dim: usize,
    k: usize,
    sweeps: usize,
) -> (Vec<Vec<f64>>, Vec<f64>) {
    let (values, vectors) = jacobi_eigen(matrix, dim, sweeps);
    let k = k.min(dim);
    (
        vectors.into_iter().take(k).collect(),
        values.into_iter().take(k).collect(),
    )
}

/// `x_k = U^T x`（`U` 的每一行是一个主方向）。
fn project(x: &[f32], comps: &[Vec<f64>]) -> Vec<f32> {
    comps
        .iter()
        .map(|u| u.iter().zip(x).map(|(a, b)| a * f64::from(*b)).sum::<f64>() as f32)
        .collect()
}

/// 降维学生：第一层 `rows × k`（`w_k[r][i] = sum_j U[i][j] w[r][j]`），读出层不变。
fn reduced_net(query: &Weights, readout: &[f32], dim: usize, comps: &[Vec<f64>]) -> Net {
    let k = comps.len();
    let base = query.to_layer().expect("ternary query");
    let bias: Vec<i8> = base.bias().iter().map(|t| t.to_i8()).collect();
    let mut trits = vec![0i8; query.rows * k];
    for row in 0..query.rows {
        let w = &query.values[row * dim..(row + 1) * dim];
        for (i, u) in comps.iter().enumerate() {
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
    let first = Layer::new(query.rows, k, &trits, &bias).expect("reduced layer");
    Net::new(vec![first, readout_layer(readout)]).expect("reduced net")
}

/// 在给定子空间上构造 oracle（输入投影 + 三值化，标签沿用全维教师）。
fn oracle_of(samples: &[Sample], comps: &[Vec<f64>]) -> Oracle {
    let inputs: Vec<Vec<Trit>> = samples
        .iter()
        .map(|s| trit_row(&project(&s.x, comps)))
        .collect();
    let outputs: Vec<Vec<Trit>> = samples.iter().map(|s| vec![s.teacher]).collect();
    Oracle::new(inputs, outputs).expect("oracle")
}

/// 纯 f32 投影标签：只保留「投影」损失，不吃「三值化」损失。
///
/// 同一个子空间上，三值路径与 f32 路径的差就是量化代价。
fn projected_label(
    x: &[f32],
    comps: &[Vec<f64>],
    query: &Weights,
    readout: &[f32],
    dim: usize,
) -> Trit {
    let xk = project(x, comps);
    let mut out = 0.0f64;
    for (row, r) in readout.iter().enumerate() {
        let w = &query.values[row * dim..(row + 1) * dim];
        let hidden: f64 = comps
            .iter()
            .zip(&xk)
            .map(|(u, xv)| {
                let wk: f64 = u.iter().zip(w).map(|(a, b)| a * f64::from(*b)).sum();
                wk * f64::from(*xv)
            })
            .sum();
        out += f64::from(*r) * hidden;
    }
    if out > 0.0 {
        Trit::Pos
    } else if out < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

fn f32_accuracy(
    samples: &[Sample],
    comps: &[Vec<f64>],
    query: &Weights,
    readout: &[f32],
    dim: usize,
) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let hit = samples
        .iter()
        .filter(|s| projected_label(&s.x, comps, query, readout, dim) == s.teacher)
        .count();
    hit as f32 / samples.len() as f32
}

fn losses(net: &Net, edit: &Oracle, keep: &Oracle) -> (usize, usize) {
    (
        edit.loss(net).expect("edit loss"),
        keep.loss(net).expect("keep loss"),
    )
}

/// 恒等基（`k == dim` 时就是原全维学生，用原输入）。
fn identity(dim: usize) -> Vec<Vec<f64>> {
    (0..dim)
        .map(|i| {
            let mut v = vec![0.0f64; dim];
            v[i] = 1.0;
            v
        })
        .collect()
}

fn main() {
    let args = parse_args();
    let query = safetensors::read_tensor(&args.safetensors, &args.query).expect("query tensor");
    let embeddings =
        safetensors::read_tensor(&args.safetensors, &args.embeddings).expect("embeddings tensor");
    assert_eq!(query.cols, embeddings.cols, "query 列数必须等于词向量维度");
    assert!(args.readout_row < query.rows, "--readout-row 越界");
    let dim = query.cols;
    let readout = &query.values[args.readout_row * dim..(args.readout_row + 1) * dim];
    let student = full_student(&query, readout);

    // 1) 采样并划分：全维三值学生判错的进 edit，判对的进 keep。
    let samples = sample_dataset(&query, &embeddings, readout, &args);
    let mut edit_samples = Vec::new();
    let mut keep_samples = Vec::new();
    let mut disagreement = 0usize;
    for sample in &samples {
        let label = student
            .forward(&trit_row(&sample.x))
            .expect("student forward")[0];
        if label == sample.teacher {
            if keep_samples.len() < args.keep_limit {
                keep_samples.push(sample.clone());
            }
        } else {
            disagreement += 1;
            if edit_samples.len() < args.edit_limit {
                edit_samples.push(sample.clone());
            }
        }
    }

    let mut report = json!({
        "safetensors": args.safetensors,
        "readout_row": args.readout_row,
        "dim": dim,
        "samples": samples.len(),
        "disagreement": disagreement,
        "edit_set": edit_samples.len(),
        "keep_set": keep_samples.len(),
        "full_trits": student.trit_count(),
    });
    if edit_samples.is_empty() || keep_samples.is_empty() {
        report["note"] = json!("edit 或 keep 集为空：三值学生与 f32 教师完全一致或完全不一致");
        emit(&args, &report);
        return;
    }

    // 2) 主方向。
    let moment = second_moment(&embeddings, dim, args.pca_samples, args.seed);
    let (comps, values) = top_components(&moment, dim, args.max_k.min(dim), args.iterations);
    let trace: f64 = (0..dim).map(|i| moment[i * dim + i]).sum();
    let total_explained: f64 = values.iter().sum();
    report["pca"] = json!({
        "components": comps.len(),
        "samples": args.pca_samples,
        "trace": trace,
        "explained_ratio": if trace > 0.0 { total_explained / trace } else { 0.0 },
        "top_values": values.iter().take(8).copied().collect::<Vec<_>>(),
    });

    // 3) 宽度曲线（三值路径 + f32 对照）。
    let mut width = Vec::new();
    let mut min_k: Option<usize> = None;
    for &k in &args.dims {
        let basis = if k >= dim {
            identity(dim)
        } else if k <= comps.len() {
            comps[..k].to_vec()
        } else {
            continue;
        };
        let net = if k >= dim {
            student.clone()
        } else {
            reduced_net(&query, readout, dim, &basis)
        };
        let edit = oracle_of(&edit_samples, &basis);
        let keep = oracle_of(&keep_samples, &basis);
        let (edit_loss, keep_loss) = losses(&net, &edit, &keep);
        if edit_loss == 0 && keep_loss == 0 && min_k.is_none_or(|best| k < best) {
            min_k = Some(k);
        }
        let (f32_edit_acc, f32_keep_acc) = if k >= dim {
            (1.0f32, 1.0f32)
        } else {
            (
                f32_accuracy(&edit_samples, &basis, &query, readout, dim),
                f32_accuracy(&keep_samples, &basis, &query, readout, dim),
            )
        };
        width.push(json!({
            "k": k,
            "edit_loss": edit_loss,
            "keep_loss": keep_loss,
            "edit_acc": edit.accuracy(&net).expect("acc"),
            "keep_acc": keep.accuracy(&net).expect("acc"),
            "f32_edit_acc": f32_edit_acc,
            "f32_keep_acc": f32_keep_acc,
            "trits": net.trit_count(),
            "reduction": net.trit_count() as f64 / student.trit_count() as f64,
        }));
    }
    report["width"] = json!(width);
    report["min_k_zero_loss"] = json!(min_k);

    // 4) 步数曲线：在降维学生上用带 keep 约束的 repair 修 edit 集。
    let mut steps = Vec::new();
    let mut minimum: Option<(usize, usize, usize)> = None;
    for &k in &args.steps_dims {
        if k >= dim || k > comps.len() {
            continue;
        }
        let sub = &comps[..k];
        let net = reduced_net(&query, readout, dim, sub);
        let edit = oracle_of(&edit_samples, sub);
        let keep = oracle_of(&keep_samples, sub);
        for &max_edits in &args.max_edits {
            for &beam in &args.beam_widths {
                let options = RepairOptions {
                    max_edits,
                    beam_width: beam,
                    exhaustive_pairs: 256,
                    exhaustive_states: 0,
                    keep: Some(keep.clone()),
                    ..RepairOptions::default()
                };
                let found = repair(&net, &edit, &options).expect("repair");
                let (edit_loss, keep_loss) = match &found {
                    Some(result) => {
                        let mut edited = net.clone();
                        result.patch.apply(&mut edited).expect("apply patch");
                        losses(&edited, &edit, &keep)
                    }
                    None => losses(&net, &edit, &keep),
                };
                if found.is_some() && edit_loss == 0 && keep_loss == 0 {
                    let key = (k, max_edits, beam);
                    if minimum.is_none_or(|best| key < best) {
                        minimum = Some(key);
                    }
                }
                steps.push(json!({
                    "k": k,
                    "max_edits": max_edits,
                    "beam_width": beam,
                    "found": found.is_some(),
                    "evaluated": found.as_ref().map(|r| r.evaluated),
                    "edit_loss": edit_loss,
                    "keep_loss": keep_loss,
                }));
            }
        }
    }
    report["steps"] = json!(steps);
    report["minimum"] = match minimum {
        Some((k, max_edits, beam)) => json!({
            "k": k,
            "max_edits": max_edits,
            "beam_width": beam,
        }),
        None => Value::Null,
    };

    // 5) 必要性：在最大子空间上逐维剔除，delta 越大越必要。
    report["necessity"] =
        necessity_ranking(&query, readout, dim, &comps, &edit_samples, &keep_samples);

    emit(&args, &report);
}

/// 逐维剔除：去掉第 `i` 个主方向后 edit+keep 的总违规增量。
fn necessity_ranking(
    query: &Weights,
    readout: &[f32],
    dim: usize,
    comps: &[Vec<f64>],
    edit_samples: &[Sample],
    keep_samples: &[Sample],
) -> Value {
    if comps.len() < 2 {
        return json!([]);
    }
    let full_net = reduced_net(query, readout, dim, comps);
    let (full_edit_loss, full_keep_loss) = losses(
        &full_net,
        &oracle_of(edit_samples, comps),
        &oracle_of(keep_samples, comps),
    );
    let baseline = full_edit_loss + full_keep_loss;
    let mut rows: Vec<(usize, usize, usize, isize)> = Vec::with_capacity(comps.len());
    for i in 0..comps.len() {
        let sub: Vec<Vec<f64>> = comps
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, v)| v.clone())
            .collect();
        let net = reduced_net(query, readout, dim, &sub);
        let (edit_loss, keep_loss) = losses(
            &net,
            &oracle_of(edit_samples, &sub),
            &oracle_of(keep_samples, &sub),
        );
        rows.push((
            i,
            edit_loss,
            keep_loss,
            (edit_loss + keep_loss) as isize - baseline as isize,
        ));
    }
    let mut sorted = rows.clone();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.3));
    let pick = |(i, e, k, d): &(usize, usize, usize, isize)| json!({"component": i, "edit_loss": e, "keep_loss": k, "delta": d});
    let most: Vec<Value> = sorted.iter().take(5).map(pick).collect();
    let least: Vec<Value> = sorted.iter().rev().take(5).map(pick).collect();
    json!({
        "k": comps.len(),
        "baseline_loss": baseline,
        "most_necessary": most,
        "least_necessary": least,
        "all": rows.iter().map(pick).collect::<Vec<_>>(),
    })
}

fn emit(args: &Args, report: &Value) {
    let text = serde_json::to_string_pretty(report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
