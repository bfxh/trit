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
use trit_wspace::safetensors;

mod basis;
mod evaluate;

use basis::*;
use evaluate::*;

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
    basis: String,
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
        basis: "pca".to_string(),
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
            "--basis" => args.basis = next(),
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
    let samples = sample_dataset(
        &query,
        &embeddings,
        readout,
        args.samples.max(args.edit_limit + args.keep_limit),
        args.seed,
    );
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
    let (pca_comps, values) = top_components(&moment, dim, args.max_k.min(dim), args.iterations);
    let (comps, criterion_cos) = if args.basis == "supervised" {
        supervised_basis(&query, readout, dim, &pca_comps, args.max_k.min(dim))
    } else {
        (pca_comps.clone(), 0.0)
    };
    report["basis"] = json!({
        "kind": args.basis,
        "criterion_cos_pca1": criterion_cos,
    });
    let trace: f64 = (0..dim).map(|i| moment[i * dim + i]).sum();
    let total_explained: f64 = values.iter().sum();
    report["pca"] = json!({
        "components": pca_comps.len(),
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
        // 全量样本上的一致性：区分「压缩了原学生」与「换成了教师」。
        let agree_full = if k >= dim {
            1.0
        } else {
            agreement_with_full(&net, &samples, &basis, &student)
        };
        width.push(json!({
            "k": k,
            "edit_loss": edit_loss,
            "keep_loss": keep_loss,
            "edit_acc": edit.accuracy(&net).expect("acc"),
            "keep_acc": keep.accuracy(&net).expect("acc"),
            "f32_edit_acc": f32_edit_acc,
            "f32_keep_acc": f32_keep_acc,
            "agree_full": agree_full,
            "trits": net.trit_count(),
            "reduction": net.trit_count() as f64 / student.trit_count() as f64,
        }));
    }
    report["width"] = json!(width);
    report["min_k_zero_loss"] = json!(min_k);

    // 4) 步数曲线：在降维学生上用带 keep 约束的 repair 修 edit 集。
    let mut steps = Vec::new();
    let mut minimum: Option<(usize, usize, usize)> = min_k.map(|k| (k, 0, 0));
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

fn emit(args: &Args, report: &Value) {
    let text = serde_json::to_string_pretty(report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
