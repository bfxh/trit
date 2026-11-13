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
use trit_wspace::safetensors;

mod basis;
mod curve;
mod evaluate;
mod judge;
mod options;

use basis::*;
use curve::*;
use evaluate::*;
use judge::*;
use options::*;

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

pub fn parse_list(value: &str) -> Vec<usize> {
    value
        .split(',')
        .map(|part| part.trim().parse().expect("列表项"))
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
    let teacher_kind = parse_teacher(&args.teacher);
    let second: Option<Vec<f32>> = if teacher_kind == Teacher::TwoTap {
        let row = (args.readout_row + 1) % query.rows;
        Some(query.values[row * dim..(row + 1) * dim].to_vec())
    } else {
        None
    };
    let samples = sample_dataset(
        &query,
        &embeddings,
        readout,
        second.as_deref(),
        teacher_kind,
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
        "teacher": args.teacher,
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

    // 3) 宽度曲线 + 4) 步数曲线。
    let cx = CurveCtx {
        args: &args,
        query: &query,
        readout,
        second: second.as_deref(),
        kind: teacher_kind,
        dim,
        comps: &comps,
        student: &student,
        samples: &samples,
        edit: &edit_samples,
        keep: &keep_samples,
    };
    let (width, min_k) = width_curve(&cx);
    report["width"] = json!(width);
    report["min_k_zero_loss"] = json!(min_k);
    let (steps, minimum) = steps_curve(&cx, min_k);
    report["steps"] = json!(steps);
    report["minimum"] = minimum;

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
