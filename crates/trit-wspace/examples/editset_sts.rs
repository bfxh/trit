//! 真句向量上的编辑集门禁：MiniLM 句向量 pair 判定 + 标准证书。
//!
//! 教师 = 人工标签（或 `cos >= threshold`），学生 = 三值化句向量 → 三值 `query` 层；
//! edit 集 = 判错的对、keep 集 = 判对的对；输出 `certificate`（`certificate.v1`），
//! `--strict` 在不成立时非零退出。
//!
//! `cargo run --release -p trit-wspace --example editset_sts -- --pairs sts-pairs.json --safetensors model.safetensors --tuned tuned.safetensors --threshold 0.5`

use serde_json::{Value, json};
use trit_wspace::certify::{Certificate, KeepReport};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Net, Trit, delta};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";

struct Args {
    pairs: String,
    safetensors: String,
    query: String,
    tuned: Option<String>,
    threshold: f32,
    strict: bool,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        pairs: String::new(),
        safetensors: String::new(),
        query: QUERY.to_string(),
        tuned: None,
        threshold: 0.5,
        strict: false,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--pairs" => args.pairs = next(),
            "--safetensors" => args.safetensors = next(),
            "--query" => args.query = next(),
            "--tuned" => args.tuned = Some(next()),
            "--threshold" => args.threshold = next().parse().expect("threshold"),
            "--strict" => args.strict = true,
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.pairs.is_empty() && !args.safetensors.is_empty());
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

/// 学生网络在全部 pair 上的逐对判定（`cos >= threshold`）。
fn judge(net: &Net, pairs: &[(Vec<Trit>, Vec<Trit>, bool)], threshold: f32) -> Vec<bool> {
    pairs
        .iter()
        .map(|(left, right, _)| {
            let a = net.forward(left).expect("forward a");
            cos_trit(&a, &net.forward(right).expect("forward b")) >= threshold
        })
        .collect()
}

/// keep / edit 两个子集分别打包成证书报告。
fn reports(want: &[bool], before: &[bool], after: &[bool]) -> (KeepReport, KeepReport) {
    let (mut keep, mut edit) = (Vec::new(), Vec::new());
    for index in 0..want.len() {
        let bucket = if before[index] == want[index] {
            &mut keep
        } else {
            &mut edit
        };
        bucket.push((want[index], before[index], after[index]));
    }
    let build = |items: &[(bool, bool, bool)]| {
        let pick = |f: fn(&(bool, bool, bool)) -> bool| items.iter().map(f).collect::<Vec<_>>();
        KeepReport::from_verdicts(&pick(|i| i.0), &pick(|i| i.1), &pick(|i| i.2), 64)
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
    let query = safetensors::read_tensor(&args.safetensors, &args.query).expect("query tensor");
    let net = Net::new(vec![query.to_layer().expect("ternary query")]).expect("student net");

    let mut pairs = Vec::with_capacity(items.len());
    let mut teacher_positive = 0;
    for item in items {
        let (a, b) = (floats(item, "a"), floats(item, "b"));
        assert_eq!(a.len(), query.cols, "句向量维度与 query 列数不一致");
        let teacher = match item.get("label").and_then(Value::as_str) {
            Some("positive") => true,
            Some("negative") => false,
            _ => cos_f32(&a, &b) >= args.threshold,
        };
        teacher_positive += usize::from(teacher);
        pairs.push((trit_row(&a), trit_row(&b), teacher));
    }

    let want: Vec<bool> = pairs.iter().map(|(_, _, teacher)| *teacher).collect();
    let before = judge(&net, &pairs, args.threshold);
    let right_baseline = want.iter().zip(&before).filter(|(w, g)| w == g).count();
    let wrong_baseline = want.len() - right_baseline;
    let mut report = json!({"pairs": pairs.len(), "threshold": args.threshold,
        "teacher_positive": teacher_positive,
        "baseline": {"wrong": wrong_baseline, "right": right_baseline}});
    let mut certificate = None;
    if let Some(tuned_path) = &args.tuned {
        let tuned = safetensors::read_tensor(tuned_path, &args.query).expect("tuned tensor");
        let (patch, base_net, stats) =
            delta::diff_weights(&query, &tuned, None, None, 1.0).expect("diff");
        let mut edited = base_net.clone();
        patch.apply(&mut edited).expect("apply");
        let after = judge(&edited, &pairs, args.threshold);
        let right_after = want.iter().zip(&after).filter(|(w, g)| w == g).count();
        let wrong_after = want.len() - right_after;
        let (keep, edit) = reports(&want, &before, &after);
        let cert = Certificate {
            keep,
            edit: Some(edit),
            patch: None,
            allow: 0,
            base_state: net.state_hash(),
            edited_state: edited.state_hash(),
        };
        report["delta"] = json!({
            "base": tuned_path,
            "changed": stats.changed(),
            "wrong": wrong_after,
            "right": right_after,
            "edit_fixed": wrong_baseline.saturating_sub(wrong_after),
            "keep_broken": right_baseline.saturating_sub(right_after),
        });
        report["certificate"] = cert.to_json();
        certificate = Some(cert);
    }
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
    if args.strict && certificate.is_some_and(|cert| !cert.passed()) {
        std::process::exit(1);
    }
}
