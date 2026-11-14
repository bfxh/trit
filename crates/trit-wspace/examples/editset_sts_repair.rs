//! 推理式编辑在真实 STS pair 判定上的验证：**不改梯度，只改离散 trit**。
//!
//! 学生 = 单层三值 `query`（384→384），输入是 MiniLM 句向量逐行三值化后的 trit。
//! 搜索分两级：
//!
//! 1. **行级探针**：把每行 trits 整体置 0，测「edit 修复数 − keep 破坏数」的净收益，
//!    选出承重行（与 `localize` 的两级定位一致）；
//! 2. **行内贪心**：在承重行内逐列试 `-1/0/+1`，每步选净收益最大的一格，
//!    走 `--max-edits` 步；每步都重新判定全部 pair。
//!
//! 输出补丁、每步读数与 `certificate.v1`；`--tuned` 可同时跑梯度差分做对照。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_sts_repair -- \
//!   --pairs sts-pairs.json --safetensors model.safetensors --max-edits 8 --rows 8
//! ```

use serde_json::{Value, json};
use trit_wspace::Trit;
use trit_wspace::certify::{Certificate, KeepReport};
use trit_wspace::safetensors::{self, Weights};

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";

struct Args {
    pairs: String,
    safetensors: String,
    query: String,
    tuned: Option<String>,
    threshold: f32,
    max_edits: usize,
    rows: usize,
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
        max_edits: 8,
        rows: 8,
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
            "--max-edits" => args.max_edits = next().parse().expect("max-edits"),
            "--rows" => args.rows = next().parse().expect("rows"),
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

fn sign(value: f32) -> Trit {
    if value > 0.0 {
        Trit::Pos
    } else if value < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

fn floats(item: &Value, key: &str) -> Vec<f32> {
    item.get(key)
        .and_then(Value::as_array)
        .expect("pairs.a/b 必须是数组")
        .iter()
        .map(|value| value.as_f64().expect("浮点数") as f32)
        .collect()
}

struct Student {
    rows: usize,
    cols: usize,
    trits: Vec<Trit>,
    scales: Vec<f32>,
}

impl Student {
    fn from_layer(layer: &trit_wspace::Layer) -> Self {
        Self {
            rows: layer.rows(),
            cols: layer.cols(),
            trits: layer.trits().to_vec(),
            scales: layer.scales().to_vec(),
        }
    }

    fn forward_row(&self, row: usize, input: &[Trit]) -> Trit {
        let mut acc = 0.0f32;
        let scale = self.scales[row];
        for (col, &value) in input.iter().enumerate() {
            let weight = f32::from(self.trits[row * self.cols + col].to_i8());
            acc += weight * scale * f32::from(value.to_i8());
        }
        sign(acc)
    }

    fn forward(&self, input: &[Trit]) -> Vec<Trit> {
        (0..self.rows)
            .map(|row| self.forward_row(row, input))
            .collect()
    }

    fn hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for trit in &self.trits {
            hash ^= u64::from(trit.to_i8() as u8);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
}

struct Pair {
    a: Vec<Trit>,
    b: Vec<Trit>,
    want: bool,
}

fn refresh_row(
    student: &Student,
    pairs: &[Pair],
    out_a: &mut [Vec<Trit>],
    out_b: &mut [Vec<Trit>],
    row: usize,
) {
    for (index, pair) in pairs.iter().enumerate() {
        out_a[index][row] = student.forward_row(row, &pair.a);
        out_b[index][row] = student.forward_row(row, &pair.b);
    }
}

fn judge(out_a: &[Vec<Trit>], out_b: &[Vec<Trit>], threshold: f32) -> Vec<bool> {
    out_a
        .iter()
        .zip(out_b)
        .map(|(a, b)| cos_trit(a, b) >= threshold)
        .collect()
}

fn counts(want: &[bool], judged: &[bool]) -> (usize, usize) {
    let right = want.iter().zip(judged).filter(|(w, g)| w == g).count();
    (right, want.len() - right)
}

/// 相对当前判定的净收益：`edit_fixed - keep_broken`。
fn net_gain(want: &[bool], before: &[bool], after: &[bool]) -> isize {
    let mut gain = 0isize;
    for index in 0..want.len() {
        let was_right = before[index] == want[index];
        let is_right = after[index] == want[index];
        if !was_right && is_right {
            gain += 1;
        } else if was_right && !is_right {
            gain -= 1;
        }
    }
    gain
}

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
    let base_layer = query.to_layer().expect("ternary query");
    let mut student = Student::from_layer(&base_layer);
    let mut pairs = Vec::with_capacity(items.len());
    for item in items {
        let (a, b) = (floats(item, "a"), floats(item, "b"));
        assert_eq!(a.len(), query.cols, "句向量维度与 query 列数不一致");
        let want = match item.get("label").and_then(Value::as_str) {
            Some("positive") => true,
            Some("negative") => false,
            _ => cos_f32(&a, &b) >= args.threshold,
        };
        pairs.push(Pair {
            a: trit_row(&a),
            b: trit_row(&b),
            want,
        });
    }
    let want: Vec<bool> = pairs.iter().map(|pair| pair.want).collect();

    // 缓存全部输出，之后改一行只刷新该行。
    let mut out_a: Vec<Vec<Trit>> = pairs.iter().map(|pair| student.forward(&pair.a)).collect();
    let mut out_b: Vec<Vec<Trit>> = pairs.iter().map(|pair| student.forward(&pair.b)).collect();
    let before = judge(&out_a, &out_b, args.threshold);
    let (right_baseline, wrong_baseline) = counts(&want, &before);
    let base_hash = student.hash();

    // 行级探针：整行置 0，测净收益。
    let mut probes = Vec::with_capacity(student.rows);
    for row in 0..student.rows {
        let saved: Vec<Trit> = (0..student.cols)
            .map(|col| student.trits[row * student.cols + col])
            .collect();
        for slot in student.trits[row * student.cols..(row + 1) * student.cols].iter_mut() {
            *slot = Trit::Zero;
        }
        refresh_row(&student, &pairs, &mut out_a, &mut out_b, row);
        let judged = judge(&out_a, &out_b, args.threshold);
        let gain = net_gain(&want, &before, &judged);
        for (col, &value) in saved.iter().enumerate() {
            student.trits[row * student.cols + col] = value;
        }
        refresh_row(&student, &pairs, &mut out_a, &mut out_b, row);
        probes.push((row, gain));
    }
    probes.sort_by_key(|(_, gain)| std::cmp::Reverse(*gain));
    probes.truncate(args.rows);

    // 行内贪心：每步在承重行内选净收益最大的一格。
    let mut current = before.clone();
    let mut edits = Vec::new();
    for _ in 0..args.max_edits {
        let mut best: Option<(usize, usize, Trit, isize)> = None;
        for &(row, _) in &probes {
            for col in 0..student.cols {
                let original = student.trits[row * student.cols + col];
                for candidate in [Trit::Neg, Trit::Zero, Trit::Pos] {
                    if candidate == original {
                        continue;
                    }
                    student.trits[row * student.cols + col] = candidate;
                    refresh_row(&student, &pairs, &mut out_a, &mut out_b, row);
                    let judged = judge(&out_a, &out_b, args.threshold);
                    let gain = net_gain(&want, &current, &judged);
                    student.trits[row * student.cols + col] = original;
                    if gain > 0 && best.is_none_or(|(_, _, _, best_gain)| gain > best_gain) {
                        best = Some((row, col, candidate, gain));
                    }
                }
                refresh_row(&student, &pairs, &mut out_a, &mut out_b, row);
            }
        }
        let Some((row, col, to, gain)) = best else {
            break;
        };
        student.trits[row * student.cols + col] = to;
        refresh_row(&student, &pairs, &mut out_a, &mut out_b, row);
        current = judge(&out_a, &out_b, args.threshold);
        let (right, wrong) = counts(&want, &current);
        edits.push(json!({
            "row": row, "col": col,
            "to": to.to_i8(),
            "net_gain": gain,
            "right": right,
            "wrong": wrong,
        }));
    }

    let after = current;
    let (right_after, wrong_after) = counts(&want, &after);
    let (keep, edit) = reports(&want, &before, &after);
    let certificate = Certificate {
        keep,
        edit: Some(edit),
        patch: None,
        allow: 0,
        base_state: base_hash,
        edited_state: student.hash(),
    };
    let mut report = json!({
        "pairs": pairs.len(),
        "threshold": args.threshold,
        "baseline": {"wrong": wrong_baseline, "right": right_baseline},
        "repair": {
            "steps": edits.len(),
            "edits": edits,
            "wrong": wrong_after,
            "right": right_after,
            "edit_fixed": wrong_baseline.saturating_sub(wrong_after),
            "keep_broken": right_baseline.saturating_sub(right_after),
        },
        "certificate": certificate.to_json(),
        "probes": probes.iter().map(|(row, gain)| json!({"row": row, "net_gain": gain})).collect::<Vec<_>>(),
    });

    // 可选：同一批 pair 上跑梯度差分的对照。
    if let Some(tuned_path) = &args.tuned {
        let tuned = safetensors::read_tensor(tuned_path, &args.query).expect("tuned tensor");
        let (patch, base_net, stats) =
            trit_wspace::delta::diff_weights(&query, &tuned, None, None, 1.0).expect("diff");
        let mut edited = base_net.clone();
        patch.apply(&mut edited).expect("apply");
        let net = edited;
        let judged: Vec<bool> = pairs
            .iter()
            .map(|pair| {
                cos_trit(
                    &net.forward(&pair.a).expect("fwd a"),
                    &net.forward(&pair.b).expect("fwd b"),
                ) >= args.threshold
            })
            .collect();
        let (right, wrong) = counts(&want, &judged);
        let (keep, edit) = reports(&want, &before, &judged);
        report["delta"] = json!({
            "base": tuned_path,
            "changed": stats.changed(),
            "wrong": wrong,
            "right": right,
            "edit_fixed": wrong_baseline.saturating_sub(wrong),
            "keep_broken": right_baseline.saturating_sub(right),
            "certificate": Certificate {
                keep,
                edit: Some(edit),
                patch: None,
                allow: 0,
                base_state: base_hash,
                edited_state: net.state_hash(),
            }.to_json(),
        });
    }

    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
    if args.strict && !certificate.passed() {
        std::process::exit(1);
    }
}
