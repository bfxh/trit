//! 6 层三值堆叠上的推理式编辑：检验「深度是否提升编辑容量」。
//!
//! 学生 = 6 层 384→384 三值线性堆叠（权重取 MiniLM 每层 `attention.self.query`），
//! 输入是句向量逐行三值化后的 trit。**这不是完整 BERT**（没有 attention/LN/残差），
//! 只用来回答「多叠 5 层三值线性，能不能比单层多修几条」。
//!
//! 搜索预算集中在**最后一层**：前 5 层输出缓存，改最后一层的 trit 只需重算该行，
//! 判定成本与单层版本同阶。行级探针 → 行内逐列贪心 → 证书。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example editset_sts_repair6 -- \
//!   --pairs sts-pairs-200.json --safetensors model.safetensors --max-edits 8 --rows 8
//! ```

use serde_json::{Value, json};
use trit_wspace::Trit;
use trit_wspace::certify::{Certificate, KeepReport};
use trit_wspace::safetensors::{self, Weights};

const LAYERS: usize = 6;
const QUERY: &str = "encoder.layer.{}.attention.self.query.weight";

struct Args {
    pairs: String,
    safetensors: String,
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

struct Stack {
    rows: usize,
    cols: usize,
    trits: Vec<Vec<Trit>>,
    scales: Vec<Vec<f32>>,
}

impl Stack {
    fn load(model: &str) -> Self {
        let mut trits = Vec::with_capacity(LAYERS);
        let mut scales = Vec::with_capacity(LAYERS);
        for layer in 0..LAYERS {
            let name = QUERY.replace("{}", &layer.to_string());
            let tensor = safetensors::read_tensor(model, &name).expect("query tensor");
            let quantized = tensor.to_layer().expect("ternary query");
            trits.push(quantized.trits().to_vec());
            scales.push(quantized.scales().to_vec());
        }
        let rows = scales[0].len();
        let cols = trits[0].len() / rows.max(1);
        Self {
            rows,
            cols,
            trits,
            scales,
        }
    }

    fn forward_layer(&self, layer: usize, input: &[Trit]) -> Vec<Trit> {
        let mut out = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            out.push(self.forward_row(layer, row, input));
        }
        out
    }

    fn forward_row(&self, layer: usize, row: usize, input: &[Trit]) -> Trit {
        let mut acc = 0.0f32;
        let scale = self.scales[layer][row];
        for (col, &value) in input.iter().enumerate() {
            let weight = f32::from(self.trits[layer][row * self.cols + col].to_i8());
            acc += weight * scale * f32::from(value.to_i8());
        }
        sign(acc)
    }

    fn forward_prefix(&self, input: &[Trit], upto: usize) -> Vec<Trit> {
        let mut current = input.to_vec();
        for layer in 0..upto {
            current = self.forward_layer(layer, &current);
        }
        current
    }

    fn hash(&self) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for layer in &self.trits {
            for trit in layer {
                hash ^= u64::from(trit.to_i8() as u8);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
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
    stack: &Stack,
    prefix: &[(Vec<Trit>, Vec<Trit>)],
    out_a: &mut [Vec<Trit>],
    out_b: &mut [Vec<Trit>],
    row: usize,
) {
    let last = LAYERS - 1;
    for (index, (pa, pb)) in prefix.iter().enumerate() {
        out_a[index][row] = stack.forward_row(last, row, pa);
        out_b[index][row] = stack.forward_row(last, row, pb);
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
    let mut stack = Stack::load(&args.safetensors);
    let mut pairs = Vec::with_capacity(items.len());
    for item in items {
        let (a, b) = (floats(item, "a"), floats(item, "b"));
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

    // 前 5 层输出缓存；最后一层输出单独维护。
    let prefix: Vec<(Vec<Trit>, Vec<Trit>)> = pairs
        .iter()
        .map(|pair| {
            (
                stack.forward_prefix(&pair.a, LAYERS - 1),
                stack.forward_prefix(&pair.b, LAYERS - 1),
            )
        })
        .collect();
    let mut out_a: Vec<Vec<Trit>> = prefix
        .iter()
        .map(|(a, _)| stack.forward_layer(LAYERS - 1, a))
        .collect();
    let mut out_b: Vec<Vec<Trit>> = prefix
        .iter()
        .map(|(_, b)| stack.forward_layer(LAYERS - 1, b))
        .collect();
    let before = judge(&out_a, &out_b, args.threshold);
    let (right_baseline, wrong_baseline) = counts(&want, &before);
    let base_hash = stack.hash();

    let last = LAYERS - 1;
    // 行级探针（只探最后一层）。
    let mut probes = Vec::with_capacity(stack.rows);
    for row in 0..stack.rows {
        let saved: Vec<Trit> = (0..stack.cols)
            .map(|col| stack.trits[last][row * stack.cols + col])
            .collect();
        for slot in stack.trits[last][row * stack.cols..(row + 1) * stack.cols].iter_mut() {
            *slot = Trit::Zero;
        }
        refresh_row(&stack, &prefix, &mut out_a, &mut out_b, row);
        let judged = judge(&out_a, &out_b, args.threshold);
        let gain = net_gain(&want, &before, &judged);
        for (col, &value) in saved.iter().enumerate() {
            stack.trits[last][row * stack.cols + col] = value;
        }
        refresh_row(&stack, &prefix, &mut out_a, &mut out_b, row);
        probes.push((row, gain));
    }
    probes.sort_by_key(|(_, gain)| std::cmp::Reverse(*gain));
    probes.truncate(args.rows);

    // 行内贪心（只改最后一层）。
    let mut current = before.clone();
    let mut edits = Vec::new();
    for _ in 0..args.max_edits {
        let mut best: Option<(usize, usize, Trit, isize)> = None;
        for &(row, _) in &probes {
            for col in 0..stack.cols {
                let original = stack.trits[last][row * stack.cols + col];
                for candidate in [Trit::Neg, Trit::Zero, Trit::Pos] {
                    if candidate == original {
                        continue;
                    }
                    stack.trits[last][row * stack.cols + col] = candidate;
                    refresh_row(&stack, &prefix, &mut out_a, &mut out_b, row);
                    let judged = judge(&out_a, &out_b, args.threshold);
                    let gain = net_gain(&want, &current, &judged);
                    stack.trits[last][row * stack.cols + col] = original;
                    if gain > 0 && best.is_none_or(|(_, _, _, best_gain)| gain > best_gain) {
                        best = Some((row, col, candidate, gain));
                    }
                }
                refresh_row(&stack, &prefix, &mut out_a, &mut out_b, row);
            }
        }
        let Some((row, col, to, gain)) = best else {
            break;
        };
        stack.trits[last][row * stack.cols + col] = to;
        refresh_row(&stack, &prefix, &mut out_a, &mut out_b, row);
        current = judge(&out_a, &out_b, args.threshold);
        let (right, wrong) = counts(&want, &current);
        edits.push(json!({
            "row": row, "col": col, "to": to.to_i8(),
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
        edited_state: stack.hash(),
    };
    let report = json!({
        "pairs": pairs.len(),
        "layers": LAYERS,
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
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
    if args.strict && !certificate.passed() {
        std::process::exit(1);
    }
}
