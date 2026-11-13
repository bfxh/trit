//! 编辑集门禁：真实 384 维输入上的「编辑达成率 / 保持集破坏率」。
//!
//! 不依赖 `transformers`（本地 torch 不可用），直接用同一个模型的两张真实张量：
//!
//! - **输入**：`embeddings.word_embeddings.weight` 的真实行向量（384 维），按 absmean 三值化；
//! - **教师**：f32 `query` 层 × 原 f32 输入，再与 f32 读出向量 r 点积取符号；
//! - **学生**：三值 `query` 层 + 三值读出层（1×384），输出 1 维三值标签；
//! - **edit 集** = 学生与教师不一致的样本（编辑必须改对）；
//! - **keep 集** = 学生已经正确的样本（编辑必须保持不变）。
//!
//! 读出向量 r 取 f32 `query` 层的第 `--readout-row` 行（真实权重，可复现）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_gate -- \
//!   --safetensors model.safetensors --tuned tuned.safetensors \
//!   --samples 512 --edit-limit 8 --keep-limit 64 --readout-row 0
//! ```
//!
//! 给了 `--tuned` 就跑 `delta::diff_weights` 生成三值补丁（只动 `query` 层），再用
//! `certify_nets` 同时检查 edit 集（必须达成）与 keep 集（必须保持）；没给则只报基线。

use serde_json::json;
use trit_wspace::certify::{CertifyOptions, certify_nets};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Layer, Net, Oracle, Trit, delta, to_wsp, wire};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";

struct Args {
    safetensors: String,
    tuned: Option<String>,
    query: String,
    embeddings: String,
    readout_row: usize,
    samples: usize,
    edit_limit: usize,
    keep_limit: usize,
    seed: u64,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        safetensors: String::new(),
        tuned: None,
        query: QUERY.to_string(),
        embeddings: EMBEDDINGS.to_string(),
        readout_row: 0,
        samples: 512,
        edit_limit: 8,
        keep_limit: 64,
        seed: 7,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--safetensors" => args.safetensors = next(),
            "--tuned" => args.tuned = Some(next()),
            "--query" => args.query = next(),
            "--embeddings" => args.embeddings = next(),
            "--readout-row" => args.readout_row = next().parse().expect("readout-row"),
            "--samples" => args.samples = next().parse().expect("samples"),
            "--edit-limit" => args.edit_limit = next().parse().expect("edit-limit"),
            "--keep-limit" => args.keep_limit = next().parse().expect("keep-limit"),
            "--seed" => args.seed = next().parse().expect("seed"),
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

/// 一行 f32 向量按 absmean 三值化（与 `Weights::to_layer` 同一实现）。
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

/// 读出层：1×dim 的三值线性层。
fn readout_layer(readout: &[f32]) -> Layer {
    Layer::new(
        1,
        readout.len(),
        &trit_row(readout)
            .iter()
            .map(|t| t.to_i8())
            .collect::<Vec<_>>(),
        &[0],
    )
    .expect("readout layer")
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

fn main() {
    let args = parse_args();
    let query = safetensors::read_tensor(&args.safetensors, &args.query).expect("query tensor");
    let embeddings =
        safetensors::read_tensor(&args.safetensors, &args.embeddings).expect("embeddings tensor");
    assert_eq!(query.cols, embeddings.cols, "query 列数必须等于词向量维度");
    assert!(
        args.readout_row < query.rows,
        "--readout-row 越界（query 有 {} 行）",
        query.rows
    );
    let dim = query.cols;
    let readout = &query.values[args.readout_row * dim..(args.readout_row + 1) * dim];
    let student = Net::new(vec![
        query.to_layer().expect("ternary query"),
        readout_layer(readout),
    ])
    .expect("student net");

    // 确定性抽样：xorshift64*，同一 seed 同结果。
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
        let student_label = student.forward(&x_trit).expect("student forward")[0];
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
        "query": args.query,
        "embeddings": args.embeddings,
        "readout_row": args.readout_row,
        "dim": dim,
        "samples": labelled,
        "disagreement": disagreement,
        "edit_set": edit_inputs.len(),
        "keep_set": keep_inputs.len(),
        "student_checksum": wire::net_summary(&student)
            .get("checksum")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string(),
    });

    if edit_inputs.is_empty() || keep_inputs.is_empty() {
        report["note"] =
            json!("edit 或 keep 集为空：三值学生在这批真实输入上与 f32 教师完全一致或完全不一致");
        emit(&args, &report);
        return;
    }
    let edit = Oracle::new(edit_inputs, edit_outputs).expect("edit oracle");
    let keep = Oracle::new(keep_inputs, keep_outputs).expect("keep oracle");
    report["baseline"] = json!({
        "edit_loss": edit.loss(&student).expect("loss"),
        "keep_loss": keep.loss(&student).expect("loss"),
    });

    let Some(tuned_path) = &args.tuned else {
        report["note"] = json!("没给 --tuned：只报基线，不跑 delta 补丁");
        emit(&args, &report);
        return;
    };

    let tuned = safetensors::read_tensor(tuned_path, &args.query).expect("tuned tensor");
    let (patch, _, stats) = delta::diff_weights(&query, &tuned, None, None, 1.0).expect("diff");
    let mut edited = student.clone();
    // 补丁只动 layer 0（query 层），读出层保持不变。
    patch.apply(&mut edited).expect("apply delta patch");
    let certificate = certify_nets(
        &student,
        &edited,
        &keep,
        Some(&edit),
        &CertifyOptions::default(),
    )
    .expect("certify");
    report["delta"] = json!({
        "base": tuned_path,
        "changed": stats.changed(),
        "changed_trits": stats.changed_trits,
        "changed_bias": stats.changed_bias,
        "certified": certificate.passed(),
        "edit": {
            "samples": certificate.edit.as_ref().map(|r| r.samples),
            "before_loss": certificate.edit.as_ref().map(|r| r.before_loss),
            "after_loss": certificate.edit.as_ref().map(|r| r.after_loss),
            "fixed": certificate.edit.as_ref().map(|r| r.fixed),
            "regressed": certificate.edit.as_ref().map(|r| r.regressed),
        },
        "keep": {
            "samples": certificate.keep.samples,
            "before_loss": certificate.keep.before_loss,
            "after_loss": certificate.keep.after_loss,
            "preserved": certificate.keep.preserved,
            "regressed": certificate.keep.regressed,
            "drifted": certificate.keep.drifted,
            "stale": certificate.keep.stale,
        },
        "edited_wsp1": to_wsp(&edited),
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
