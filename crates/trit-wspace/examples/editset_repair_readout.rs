//! `localize` 选承重行 + `repair` 的 scoped 束搜索：把 #477 的“不可行”补成真实读数。
//!
//! 任务与 `editset_gate` 相同（`word_embeddings` 行做输入、f32 `query` 层做教师、
//! 三值 `query` + 三值读出层做学生），流程：
//!
//! 1. `localize` 选承重行 → 展开成候选（行内所有 trit 的另两个取值 + bias）；
//! 2. `repair` 带 `scope` 做束搜索 → `certify_nets` 检查 edit 达成与 keep 保持。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_repair_readout -- \
//!   --safetensors model.safetensors --edit-limit 8 --keep-limit 64 --top-rows 8 --max-edits 2 --beam-width 16
//! ```

use serde_json::json;
use trit_wspace::certify::{CertifyOptions, certify_nets};
use trit_wspace::localize;
use trit_wspace::repair::{RepairOptions, repair};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Edit, Layer, Net, Oracle, Trit};

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
    row_budget: usize,
    top_rows: usize,
    max_trits: usize,
    top_trits: usize,
    max_edits: usize,
    beam_width: usize,
    row_ops: bool,
    out: Option<String>,
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
        row_budget: 64,
        top_rows: 8,
        max_trits: 64,
        top_trits: 32,
        max_edits: 2,
        beam_width: 16,
        row_ops: false,
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
            "--row-budget" => args.row_budget = next().parse().expect("row-budget"),
            "--top-rows" => args.top_rows = next().parse().expect("top-rows"),
            "--max-trits" => args.max_trits = next().parse().expect("max-trits"),
            "--top-trits" => args.top_trits = next().parse().expect("top-trits"),
            "--max-edits" => args.max_edits = next().parse().expect("max-edits"),
            "--row-ops" => args.row_ops = true,
            "--beam-width" => args.beam_width = next().parse().expect("beam-width"),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.safetensors.is_empty(), "需要 --safetensors");
    args
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

fn readout_layer(readout: &[f32]) -> Layer {
    let trits: Vec<i8> = trit_row(readout).iter().map(|t| t.to_i8()).collect();
    Layer::new(1, readout.len(), &trits, &[0]).expect("readout layer")
}

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

/// 把 (layer, row) 展开成该行所有 trit（另两个取值）与 bias 的候选。
fn row_scope(net: &Net, rows: &[(usize, usize)]) -> Vec<Edit> {
    let mut out = Vec::new();
    for &(layer, row) in rows {
        let Some(target) = net.layer(layer) else {
            continue;
        };
        if row >= target.rows() {
            continue;
        }
        for col in 0..target.cols() {
            let current = target.trit(row, col).expect("trit");
            for &to in &Trit::ALL {
                if to != current {
                    out.push(Edit::SetTrit {
                        layer,
                        row,
                        col,
                        to,
                    });
                }
            }
        }
        let current = target.bias_trit(row).expect("bias");
        for &to in &Trit::ALL {
            if to != current {
                out.push(Edit::SetBias { layer, row, to });
            }
        }
    }
    out
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
    let student = Net::new(vec![
        query.to_layer().expect("ternary query"),
        readout_layer(readout),
    ])
    .expect("student net");

    let mut state = args.seed ^ 0x9E37_79B9_7F4A_7C15;
    if state == 0 {
        state = 1;
    }
    let mut next = || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };

    let budget = args.samples.max(args.edit_limit + args.keep_limit);
    let mut labelled = 0usize;
    let mut disagreement = 0usize;
    let mut edit_inputs = Vec::new();
    let mut edit_outputs = Vec::new();
    let mut keep_inputs = Vec::new();
    let mut keep_outputs = Vec::new();
    while labelled < budget {
        let row = (next() % embeddings.rows as u64) as usize;
        let x_f32 = &embeddings.values[row * dim..(row + 1) * dim];
        let teacher = sign_of(dot(readout, &teacher_forward(&query, x_f32)));
        if teacher == Trit::Zero {
            continue;
        }
        labelled += 1;
        let x_trit = trit_row(x_f32);
        let student_label = student.forward(&x_trit).expect("forward")[0];
        if student_label == teacher {
            if keep_inputs.len() < args.keep_limit {
                keep_inputs.push(x_trit);
                keep_outputs.push(vec![teacher]);
            }
        } else {
            disagreement += 1;
            if edit_inputs.len() < args.edit_limit {
                edit_inputs.push(x_trit);
                edit_outputs.push(vec![teacher]);
            }
        }
        if edit_inputs.len() >= args.edit_limit && keep_inputs.len() >= args.keep_limit {
            break;
        }
    }

    let mut report = json!({
        "safetensors": args.safetensors,
        "readout_row": args.readout_row,
        "samples": labelled,
        "disagreement": disagreement,
        "edit_set": edit_inputs.len(),
        "keep_set": keep_inputs.len(),
    });
    if edit_inputs.is_empty() || keep_inputs.is_empty() {
        report["note"] = json!("edit 或 keep 集为空");
        emit(&args, &report);
        return;
    }
    let edit = Oracle::new(edit_inputs, edit_outputs).expect("edit oracle");
    let keep = Oracle::new(keep_inputs, keep_outputs).expect("keep oracle");
    report["baseline"] = json!({
        "edit_loss": edit.loss(&student).expect("loss"),
        "keep_loss": keep.loss(&student).expect("loss"),
    });

    let located = localize::localize(
        &student,
        &edit,
        args.row_budget,
        args.top_rows,
        args.max_trits,
        args.top_trits,
    )
    .expect("localize");
    let rows: Vec<(usize, usize)> = located
        .rows
        .iter()
        .map(|row| (row.layer, row.row))
        .collect();
    let scope = row_scope(&student, &rows);
    report["localize"] = json!({
        "rows": rows.len(),
        "row_list": rows,
        "candidates": scope.len(),
        "probed": located.rows.iter().map(|row| row.probed).sum::<usize>(),
    });
    if scope.is_empty() {
        report["note"] = json!("localize 没有选出候选行");
        emit(&args, &report);
        return;
    }

    let options = RepairOptions {
        max_edits: args.max_edits,
        beam_width: args.beam_width,
        scope: Some(scope),
        row_ops: args.row_ops,
        ..RepairOptions::default()
    };
    let Some(found) = repair(&student, &edit, &options).expect("repair") else {
        report["repair"] = json!({"found": false, "budget_exhausted": true});
        emit(&args, &report);
        return;
    };
    let mut edited = student.clone();
    found.patch.apply(&mut edited).expect("apply");
    let certificate = certify_nets(
        &student,
        &edited,
        &keep,
        Some(&edit),
        &CertifyOptions::default(),
    )
    .expect("certify");
    report["repair"] = json!({
        "found": true,
        "edits": found.patch.len(),
        "evaluated": found.evaluated,
        "before_loss": found.before_loss,
        "after_loss": found.after_loss,
        "certified": certificate.passed(),
        "edit": {
            "samples": certificate.edit.as_ref().map(|r| r.samples),
            "before_loss": certificate.edit.as_ref().map(|r| r.before_loss),
            "after_loss": certificate.edit.as_ref().map(|r| r.after_loss),
            "fixed": certificate.edit.as_ref().map(|r| r.fixed),
        },
        "keep": {
            "samples": certificate.keep.samples,
            "before_loss": certificate.keep.before_loss,
            "after_loss": certificate.keep.after_loss,
            "preserved": certificate.keep.preserved,
            "regressed": certificate.keep.regressed,
        },
        "patch": trit_wspace::wire::patch_to_value(&found.patch),
    });
    emit(&args, &report);
}

fn emit(args: &Args, report: &serde_json::Value) {
    let text = serde_json::to_string_pretty(report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
