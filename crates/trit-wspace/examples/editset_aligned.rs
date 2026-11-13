//! 阈值对齐后的单层 / 两层对照：在 valid 上选阈值，再在 test 上报告。
//!
//! 固定阈值下两个网络不可比（单层的有利区间在 0.3，两层在 0.5）。这里按
//! “valid 上 baseline 准确率最高”的阈值对齐，再在 test 上比较 edit_fixed / keep_broken：
//!
//! - 单层：三值 `query`（384→384）；
//! - 两层：三值 `intermediate.dense` + `output.dense`（384→1536→384）；
//! - 判定：两个句向量在输出空间的余弦 `>= threshold`；
//! - valid / test：按 pairs 索引切分（前 `valid-ratio` 为 valid）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_aligned -- --pairs sts-bench.json \
//!   --safetensors model.safetensors --tuned tuned.safetensors --arch ffn --valid-ratio 0.3
//! ```

use serde_json::{Value, json};
use trit_wspace::certify::KeepReport;
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Edit, Net, Patch, Trit, delta};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const T1: &str = "encoder.layer.0.intermediate.dense.weight";
const T2: &str = "encoder.layer.0.output.dense.weight";

struct Args {
    pairs: String,
    safetensors: String,
    tuned: Option<String>,
    arch: String,
    valid_ratio: f32,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        pairs: String::new(),
        safetensors: String::new(),
        tuned: None,
        arch: "ffn".to_string(),
        valid_ratio: 0.3,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--pairs" => args.pairs = next(),
            "--safetensors" => args.safetensors = next(),
            "--tuned" => args.tuned = Some(next()),
            "--arch" => args.arch = next(),
            "--valid-ratio" => args.valid_ratio = next().parse().expect("valid-ratio"),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.pairs.is_empty() && !args.safetensors.is_empty());
    assert!(
        args.arch == "single" || args.arch == "ffn",
        "--arch 只支持 single/ffn"
    );
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

fn floats(item: &Value, key: &str) -> Vec<f32> {
    let values = item
        .get(key)
        .and_then(Value::as_array)
        .expect("pairs.a/b 必须是数组");
    values
        .iter()
        .map(|value| value.as_f64().expect("浮点数") as f32)
        .collect()
}

fn base_net(args: &Args) -> Net {
    if args.arch == "single" {
        let w = safetensors::read_tensor(&args.safetensors, QUERY).expect("query");
        Net::new(vec![w.to_layer().expect("ternary query")]).expect("single net")
    } else {
        let w1 = safetensors::read_tensor(&args.safetensors, T1).expect("intermediate");
        let w2 = safetensors::read_tensor(&args.safetensors, T2).expect("output");
        Net::new(vec![
            w1.to_layer().expect("ternary intermediate"),
            w2.to_layer().expect("ternary output"),
        ])
        .expect("two-layer net")
    }
}

fn shift(edit: &Edit) -> Edit {
    match edit {
        Edit::SetTrit {
            layer,
            row,
            col,
            to,
        } => Edit::SetTrit {
            layer: layer + 1,
            row: *row,
            col: *col,
            to: *to,
        },
        Edit::SetBias { layer, row, to } => Edit::SetBias {
            layer: layer + 1,
            row: *row,
            to: *to,
        },
        Edit::SetScale { layer, row, to } => Edit::SetScale {
            layer: layer + 1,
            row: *row,
            to: *to,
        },
        other => (*other).clone(),
    }
}

/// 应用 tuned 差分，返回 (edited net, 三值变化数)。
fn apply_tuned(args: &Args, net: &Net) -> (Net, usize) {
    let path = args.tuned.as_ref().expect("tuned");
    if args.arch == "single" {
        let base = safetensors::read_tensor(&args.safetensors, QUERY).expect("query");
        let tuned = safetensors::read_tensor(path, QUERY).expect("tuned query");
        let (patch, base_net, stats) =
            delta::diff_weights(&base, &tuned, None, None, 1.0).expect("diff");
        let mut edited = base_net.clone();
        patch.apply(&mut edited).expect("apply");
        (edited, stats.changed())
    } else {
        let base1 = safetensors::read_tensor(&args.safetensors, T1).expect("intermediate");
        let base2 = safetensors::read_tensor(&args.safetensors, T2).expect("output");
        let t1 = safetensors::read_tensor(path, T1).expect("tuned intermediate");
        let t2 = safetensors::read_tensor(path, T2).expect("tuned output");
        let (patch1, base_net1, stats1) =
            delta::diff_weights(&base1, &t1, None, None, 1.0).expect("diff 0");
        let (patch2, base_net2, stats2) =
            delta::diff_weights(&base2, &t2, None, None, 1.0).expect("diff 1");
        let mut edits: Vec<Edit> = patch1.edits().to_vec();
        edits.extend(patch2.edits().iter().map(shift));
        let mut edited = Net::new(vec![
            base_net1.layers()[0].clone(),
            base_net2.layers()[0].clone(),
        ])
        .expect("base net");
        Patch::new(edits).apply(&mut edited).expect("apply");
        let _ = net;
        (edited, stats1.changed() + stats2.changed())
    }
}

fn scores(net: &Net, pairs: &[(Vec<Trit>, Vec<Trit>, bool)]) -> Vec<f32> {
    pairs
        .iter()
        .map(|(left, right, _)| {
            let a = net.forward(left).expect("forward a");
            cos_trit(&a, &net.forward(right).expect("forward b"))
        })
        .collect()
}

fn split(len: usize, ratio: f32) -> (Vec<usize>, Vec<usize>) {
    let valid = ((len as f32) * ratio).round() as usize;
    ((0..valid).collect(), (valid..len).collect())
}

/// 在 valid 索引上扫阈值，返回 (最佳阈值, baseline 准确率)。
fn best_threshold(want: &[bool], scores: &[f32], index: &[usize]) -> (f32, f32) {
    let grid: Vec<f32> = (1..=95).map(|step| step as f32 / 100.0).collect();
    let mut best = (0.5f32, 0.0f32);
    for &threshold in &grid {
        let right = index
            .iter()
            .filter(|&&i| (scores[i] >= threshold) == want[i])
            .count();
        let accuracy = right as f32 / index.len() as f32;
        if accuracy > best.1 {
            best = (threshold, accuracy);
        }
    }
    best
}

fn counts(want: &[bool], scores: &[f32], threshold: f32, index: &[usize]) -> (usize, usize) {
    let right = index
        .iter()
        .filter(|&&i| (scores[i] >= threshold) == want[i])
        .count();
    (index.len() - right, right)
}

fn reports(
    want: &[bool],
    before: &[bool],
    after: &[bool],
    index: &[usize],
) -> (KeepReport, KeepReport) {
    let (mut keep, mut edit) = (Vec::new(), Vec::new());
    for &i in index {
        let bucket = if before[i] == want[i] {
            &mut keep
        } else {
            &mut edit
        };
        bucket.push((want[i], before[i], after[i]));
    }
    let build = |items: &[(bool, bool, bool)]| {
        let pick = |f: fn(&(bool, bool, bool)) -> bool| items.iter().map(f).collect::<Vec<_>>();
        KeepReport::from_verdicts(&pick(|x| x.0), &pick(|x| x.1), &pick(|x| x.2), 64)
    };
    (build(&keep), build(&edit))
}

fn main() {
    let args = parse_args();
    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&args.pairs).expect("pairs")).expect("json");
    let items = payload
        .get("pairs")
        .and_then(Value::as_array)
        .expect("pairs 数组");
    let net = base_net(&args);
    let mut pairs = Vec::with_capacity(items.len());
    let mut want = Vec::with_capacity(items.len());
    for item in items {
        let (a, b) = (floats(item, "a"), floats(item, "b"));
        let teacher = match item.get("label").and_then(Value::as_str) {
            Some("positive") => true,
            Some("negative") => false,
            _ => cos_f32(&a, &b) >= 0.5,
        };
        want.push(teacher);
        pairs.push((trit_row(&a), trit_row(&b), teacher));
    }

    let before_scores = scores(&net, &pairs);
    let (valid, test) = split(pairs.len(), args.valid_ratio);
    let (threshold, valid_accuracy) = best_threshold(&want, &before_scores, &valid);
    let before_bool: Vec<bool> = before_scores.iter().map(|&s| s >= threshold).collect();

    let mut report = json!({
        "arch": args.arch,
        "pairs": pairs.len(),
        "valid_pairs": valid.len(),
        "test_pairs": test.len(),
        "threshold": threshold,
        "valid_accuracy": valid_accuracy,
    });
    if args.tuned.is_some() {
        let (edited, changed) = apply_tuned(&args, &net);
        let after_scores = scores(&edited, &pairs);
        let after_bool: Vec<bool> = after_scores.iter().map(|&s| s >= threshold).collect();
        let (wrong, right) = counts(&want, &before_scores, threshold, &test);
        let (wrong_after, right_after) = counts(&want, &after_scores, threshold, &test);
        let (keep, edit) = reports(&want, &before_bool, &after_bool, &test);
        report["delta"] = json!({
            "changed": changed,
            "test_baseline": {"wrong": wrong, "right": right},
            "test_after": {"wrong": wrong_after, "right": right_after},
            "edit_fixed": wrong.saturating_sub(wrong_after),
            "keep_broken": right.saturating_sub(right_after),
            "certificate": {"keep": keep.to_json(), "edit": edit.to_json()},
        });
    }
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
