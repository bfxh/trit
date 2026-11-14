//! grouped 表示上的推理式编辑：**f32 激活 + 三值权重**，搜索按单 trit（组 scale 固定）。
//!
//! 与前两个门禁的区别：
//!
//! - `editset_sts` / `editset_sts_repair`：学生是纯三值单层，输入也要逐行三值化（丢激活精度）；
//! - 本示例：学生是 `GroupedLayer`（每 G 个权重共享一个 f32 scale），**输入保留原始 384 维
//!   句向量**，输出是 f32，判定用 cosine ——这对应 `WSP-SUMMARY` §5 的「精度救回来了」。
//!
//! 搜索仍是行级探针 → 行内逐列试 `-1/0/+1`，按「edit 修复数 − keep 破坏数」贪心。
//! 改一个 trit 只影响一行输出，所以缓存每对的输出、只重算该行。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_sts_grouped_repair -- \
//!   --pairs sts-pairs-200.json --safetensors model.safetensors \
//!   --group 256 --max-edits 8 --rows 8
//! ```

use serde_json::{Value, json};
use trit_wspace::certify::{Certificate, KeepReport};
use trit_wspace::grouped::GroupedLayer;
use trit_wspace::safetensors;

const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const QUERY_BIAS: &str = "encoder.layer.0.attention.self.query.bias";

struct Args {
    pairs: String,
    safetensors: String,
    group: usize,
    threshold: f32,
    max_edits: usize,
    rows: usize,
    out: Option<String>,
    strict: bool,
}

fn parse_args() -> Args {
    let mut args = Args {
        pairs: String::new(),
        safetensors: String::new(),
        group: 256,
        threshold: 0.5,
        max_edits: 8,
        rows: 8,
        out: None,
        strict: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--pairs" => args.pairs = next(),
            "--safetensors" => args.safetensors = next(),
            "--group" => args.group = next().parse().expect("group"),
            "--threshold" => args.threshold = next().parse().expect("threshold"),
            "--max-edits" => args.max_edits = next().parse().expect("max-edits"),
            "--rows" => args.rows = next().parse().expect("rows"),
            "--out" => args.out = Some(next()),
            "--strict" => args.strict = true,
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.pairs.is_empty() && !args.safetensors.is_empty());
    args
}

fn cos_f32(left: &[f32], right: &[f32]) -> f32 {
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let norm = left.iter().map(|v| v * v).sum::<f32>().sqrt()
        * right.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 { 0.0 } else { dot / norm }
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
    trits: Vec<i8>,
    groups: Vec<u32>,
    scales: Vec<f32>,
}

impl Student {
    fn from_layer(layer: &GroupedLayer) -> Self {
        let (rows, cols) = (layer.rows(), layer.cols());
        let mut trits = Vec::with_capacity(rows * cols);
        let mut groups = Vec::with_capacity(rows * cols);
        for row in 0..rows {
            for col in 0..cols {
                trits.push(layer.trit(row, col).expect("trit").to_i8());
                groups.push(layer.group_of(row, col).expect("group"));
            }
        }
        Self {
            rows,
            cols,
            trits,
            groups,
            scales: layer.scales().to_vec(),
        }
    }

    fn weight(&self, row: usize, col: usize) -> f32 {
        let index = row * self.cols + col;
        f32::from(self.trits[index]) * self.scales[self.groups[index] as usize]
    }

    fn forward_row(&self, row: usize, input: &[f32]) -> f32 {
        let mut acc = 0.0f64;
        for (col, &value) in input.iter().enumerate() {
            acc += f64::from(self.weight(row, col)) * f64::from(value);
        }
        acc as f32
    }

    fn forward(&self, input: &[f32]) -> Vec<f32> {
        (0..self.rows)
            .map(|row| self.forward_row(row, input))
            .collect()
    }

    fn hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for value in &self.trits {
            hash ^= u64::from(*value as u8);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        for value in &self.scales {
            for byte in value.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }
}

struct Pair {
    a: Vec<f32>,
    b: Vec<f32>,
    want: bool,
}

fn refresh_row(
    student: &Student,
    pairs: &[Pair],
    out_a: &mut [Vec<f32>],
    out_b: &mut [Vec<f32>],
    row: usize,
) {
    for (index, pair) in pairs.iter().enumerate() {
        out_a[index][row] = student.forward_row(row, &pair.a);
        out_b[index][row] = student.forward_row(row, &pair.b);
    }
}

fn judge(out_a: &[Vec<f32>], out_b: &[Vec<f32>], threshold: f32) -> Vec<bool> {
    out_a
        .iter()
        .zip(out_b)
        .map(|(a, b)| cos_f32(a, b) >= threshold)
        .collect()
}

fn counts(want: &[bool], judged: &[bool]) -> (usize, usize) {
    let right = want.iter().zip(judged).filter(|(w, g)| w == g).count();
    (right, want.len() - right)
}

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
    let query = safetensors::read_tensor(&args.safetensors, QUERY).expect("query tensor");
    let bias = safetensors::read_vector(&args.safetensors, QUERY_BIAS)
        .unwrap_or_else(|_| vec![0.0; query.rows]);
    let layer = GroupedLayer::quantize(query.rows, query.cols, &query.values, &bias, args.group)
        .expect("grouped quantize");
    let mut student = Student::from_layer(&layer);
    let mut pairs = Vec::with_capacity(items.len());
    for item in items {
        let (a, b) = (floats(item, "a"), floats(item, "b"));
        let want = match item.get("label").and_then(Value::as_str) {
            Some("positive") => true,
            Some("negative") => false,
            _ => cos_f32(&a, &b) >= args.threshold,
        };
        pairs.push(Pair { a, b, want });
    }
    let want: Vec<bool> = pairs.iter().map(|pair| pair.want).collect();

    let mut out_a: Vec<Vec<f32>> = pairs.iter().map(|pair| student.forward(&pair.a)).collect();
    let mut out_b: Vec<Vec<f32>> = pairs.iter().map(|pair| student.forward(&pair.b)).collect();
    let before = judge(&out_a, &out_b, args.threshold);
    let (right_baseline, wrong_baseline) = counts(&want, &before);
    let base_hash = student.hash();

    // 行级探针：整行 trits 置 0，测净收益。
    let mut probes = Vec::with_capacity(student.rows);
    for row in 0..student.rows {
        let saved: Vec<i8> = (0..student.cols)
            .map(|col| student.trits[row * student.cols + col])
            .collect();
        for slot in student.trits[row * student.cols..(row + 1) * student.cols].iter_mut() {
            *slot = 0;
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

    // 行内贪心：每组候选改一个 trit。
    let mut current = before.clone();
    let mut edits = Vec::new();
    for _ in 0..args.max_edits {
        let mut best: Option<(usize, usize, i8, isize)> = None;
        for &(row, _) in &probes {
            for col in 0..student.cols {
                let original = student.trits[row * student.cols + col];
                for candidate in [-1i8, 0, 1] {
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
            "row": row, "col": col, "to": to,
            "net_gain": gain, "right": right, "wrong": wrong,
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
    let report = json!({
        "pairs": pairs.len(),
        "group": args.group,
        "threshold": args.threshold,
        "input": "f32 sentence vectors",
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
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
    if args.strict && !certificate.passed() {
        std::process::exit(1);
    }
}
