//! 真句向量上的编辑集门禁：用本地 MiniLM 编码的真实句向量做 pair 判定。
//!
//! 与 `editset_gate` 的区别：输入是 `transformers` 编码出的 384 维**句向量**
//! （由 `encode_sts_pairs.py` 生成 pairs JSON），判定是两个句向量在 `query` 层
//! 输出空间里的余弦：教师用全精度 `cos(a, b) >= threshold`，学生用三值化句向量；
//! edit 集 = 学生当前判错的对（必须改对），keep 集 = 判对的对（必须保持不变）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_sts -- --pairs sts-pairs.json \
//!   --safetensors model.safetensors --tuned tuned.safetensors --threshold 0.5
//! ```

use serde_json::{Value, json};
use trit_wspace::safetensors::{self, Weights};
use trit_wspace::{Net, Trit, delta};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";

struct Args {
    pairs: String,
    safetensors: String,
    query: String,
    tuned: Option<String>,
    threshold: f32,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        pairs: String::new(),
        safetensors: String::new(),
        query: QUERY.to_string(),
        tuned: None,
        threshold: 0.5,
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
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.pairs.is_empty(), "需要 --pairs");
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
    let norm_left: f32 = left.iter().map(|v| v * v).sum::<f32>().sqrt();
    let norm_right: f32 = right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm_left == 0.0 || norm_right == 0.0 {
        0.0
    } else {
        dot / (norm_left * norm_right)
    }
}

fn cos_trit(left: &[Trit], right: &[Trit]) -> f32 {
    let mut dot = 0i64;
    let mut sum_left = 0i64;
    let mut sum_right = 0i64;
    for (a, b) in left.iter().zip(right) {
        let x = i64::from(a.to_i8());
        let y = i64::from(b.to_i8());
        dot += x * y;
        sum_left += x * x;
        sum_right += y * y;
    }
    if sum_left == 0 || sum_right == 0 {
        0.0
    } else {
        dot as f32 / ((sum_left as f32).sqrt() * (sum_right as f32).sqrt())
    }
}

fn vec(value: &Value, key: &str) -> Vec<f32> {
    value
        .get(key)
        .and_then(Value::as_array)
        .expect("pairs.a/b 必须是数组")
        .iter()
        .map(|item| item.as_f64().expect("浮点数") as f32)
        .collect()
}

/// 一个网络在全部 pair 上的 (判错数, 判对数)。
fn score(net: &Net, pairs: &[(Vec<Trit>, Vec<Trit>, bool)], threshold: f32) -> (usize, usize) {
    let mut wrong = 0;
    let mut right = 0;
    for (left, right_row, teacher) in pairs {
        let y_left = net.forward(left).expect("forward a");
        let y_right = net.forward(right_row).expect("forward b");
        let student = cos_trit(&y_left, &y_right) >= threshold;
        if student == *teacher {
            right += 1;
        } else {
            wrong += 1;
        }
    }
    (wrong, right)
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
        let a = vec(item, "a");
        let b = vec(item, "b");
        assert_eq!(a.len(), query.cols, "句向量维度与 query 列数不一致");
        let teacher = match item.get("label").and_then(Value::as_str) {
            Some("positive") => true,
            Some("negative") => false,
            _ => cos_f32(&a, &b) >= args.threshold,
        };
        if teacher {
            teacher_positive += 1;
        }
        pairs.push((trit_row(&a), trit_row(&b), teacher));
    }

    let (wrong_baseline, right_baseline) = score(&net, &pairs, args.threshold);
    let mut report = json!({
        "pairs": pairs.len(),
        "threshold": args.threshold,
        "teacher_positive": teacher_positive,
        "baseline": {
            "wrong": wrong_baseline,
            "right": right_baseline,
        },
    });

    let Some(tuned_path) = &args.tuned else {
        emit(&args, &report);
        return;
    };
    let tuned = safetensors::read_tensor(tuned_path, &args.query).expect("tuned tensor");
    let (patch, base_net, stats) =
        delta::diff_weights(&query, &tuned, None, None, 1.0).expect("diff");
    let mut edited = base_net.clone();
    patch.apply(&mut edited).expect("apply");
    let (wrong_after, right_after) = score(&edited, &pairs, args.threshold);
    report["delta"] = json!({
        "base": tuned_path,
        "changed": stats.changed(),
        "wrong": wrong_after,
        "right": right_after,
        "edit_fixed": wrong_baseline.saturating_sub(wrong_after),
        "keep_broken": right_baseline.saturating_sub(right_after),
    });
    emit(&args, &report);
}

fn emit(args: &Args, report: &Value) {
    let text = serde_json::to_string_pretty(report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
