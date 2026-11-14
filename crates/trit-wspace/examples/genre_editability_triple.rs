//! 三 trit 组合的上界（剪枝版）：单/双都修不了的错样本，三个 trit 能多修多少。
//!
//! 精确枚举 `C(2304,3) ≈ 20 亿` 组合 × 114 个样本 ≈ 2.3e11 次判断，单线程要十几到几十分钟。
//! 这里用 **top-K 候选剪枝**：按「目标类增益 − 竞争类最大增益」排序，只保留前 `--top-k`
//! 个候选（默认 512），再枚举三组合（`C(512,3) ≈ 2200 万`）。**输出的是剪枝后的下界**，
//! 不是精确上界；`--top-k 0` 时退回精确枚举（慢）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example genre_editability_triple -- \
//!   --samples genre-samples.json --group 384 --top-k 512
//! ```

use serde_json::{Value, json};
use trit_wspace::grouped::GroupedLayer;

const CLASSES: usize = 3;

struct Args {
    samples: String,
    group: usize,
    top_k: usize,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        samples: String::new(),
        group: 384,
        top_k: 512,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--samples" => args.samples = next(),
            "--group" => args.group = next().parse().expect("group"),
            "--top-k" => args.top_k = next().parse().expect("top-k"),
            "--out" => args.out = Some(next()),
            other => panic!("未知参数 {other}"),
        }
    }
    assert!(!args.samples.is_empty());
    args
}

struct Sample {
    vector: Vec<f32>,
    label: usize,
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

    fn scores(&self, input: &[f32]) -> [f32; CLASSES] {
        let mut out = [0.0f32; CLASSES];
        for (row, slot) in out.iter_mut().enumerate() {
            let mut acc = 0.0f64;
            for (col, &value) in input.iter().enumerate() {
                acc += f64::from(self.weight(row, col)) * f64::from(value);
            }
            *slot = acc as f32;
        }
        out
    }

    fn argmax(scores: &[f32; CLASSES]) -> usize {
        let mut best = 0usize;
        for index in 1..CLASSES {
            if scores[index] > scores[best] {
                best = index;
            }
        }
        best
    }

    fn candidates(&self, input: &[f32]) -> Vec<(usize, i8, [f32; CLASSES])> {
        let mut out = Vec::with_capacity(self.rows * self.cols * 2);
        for row in 0..self.rows {
            let scale = self.scales[row];
            for (col, &value) in input.iter().enumerate() {
                let pos = row * self.cols + col;
                let current = self.trits[pos];
                for to in [-1i8, 0, 1] {
                    if to == current {
                        continue;
                    }
                    let mut delta = [0.0f32; CLASSES];
                    delta[row] = (f32::from(to) - f32::from(current)) * scale * value;
                    out.push((pos, to, delta));
                }
            }
        }
        out
    }
}

fn parse_samples(value: &Value) -> Vec<Sample> {
    value
        .as_array()
        .expect("samples 数组")
        .iter()
        .map(|item| Sample {
            vector: item
                .get("vector")
                .and_then(Value::as_array)
                .expect("vector")
                .iter()
                .map(|v| v.as_f64().expect("float") as f32)
                .collect(),
            label: item.get("label").and_then(Value::as_u64).expect("label") as usize,
        })
        .collect()
}

fn centroids(train: &[Sample], cols: usize) -> Vec<f32> {
    let mut sums = vec![0.0f32; CLASSES * cols];
    let mut counts = [0usize; CLASSES];
    for sample in train {
        let offset = sample.label * cols;
        for (col, &value) in sample.vector.iter().enumerate() {
            sums[offset + col] += value;
        }
        counts[sample.label] += 1;
    }
    for label in 0..CLASSES {
        let count = counts[label].max(1) as f32;
        for col in 0..cols {
            sums[label * cols + col] /= count;
        }
    }
    sums
}

fn single_reachable(student: &Student, sample: &Sample) -> bool {
    let base = student.scores(&sample.vector);
    for (_, _, delta) in student.candidates(&sample.vector) {
        let mut scores = base;
        for (slot, value) in scores.iter_mut().enumerate() {
            *value += delta[slot];
        }
        if Student::argmax(&scores) == sample.label {
            return true;
        }
    }
    false
}

fn pair_reachable(student: &Student, sample: &Sample) -> bool {
    let base = student.scores(&sample.vector);
    let candidates = student.candidates(&sample.vector);
    for left in 0..candidates.len() {
        for right in (left + 1)..candidates.len() {
            if candidates[left].0 == candidates[right].0 {
                continue;
            }
            let mut scores = base;
            for (slot, value) in scores.iter_mut().enumerate() {
                *value += candidates[left].2[slot] + candidates[right].2[slot];
            }
            if Student::argmax(&scores) == sample.label {
                return true;
            }
        }
    }
    false
}

/// 候选的「目标类增益 − 竞争类最大增益」——越正越有利于把样本拉回目标类。
fn target_score(candidate: &(usize, i8, [f32; CLASSES]), label: usize) -> f32 {
    candidate.2[label]
        - (0..CLASSES)
            .filter(|&row| row != label)
            .map(|row| candidate.2[row])
            .fold(f32::NEG_INFINITY, f32::max)
}

fn triple_reachable(student: &Student, sample: &Sample, top_k: usize) -> bool {
    let base = student.scores(&sample.vector);
    let mut candidates = student.candidates(&sample.vector);
    if top_k > 0 && candidates.len() > top_k {
        candidates.sort_by(|a, b| {
            target_score(b, sample.label)
                .partial_cmp(&target_score(a, sample.label))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates.truncate(top_k);
    }
    for left in 0..candidates.len() {
        for right in (left + 1)..candidates.len() {
            if candidates[left].0 == candidates[right].0 {
                continue;
            }
            for third in (right + 1)..candidates.len() {
                if candidates[third].0 == candidates[left].0
                    || candidates[third].0 == candidates[right].0
                {
                    continue;
                }
                let mut scores = base;
                for (slot, value) in scores.iter_mut().enumerate() {
                    *value += candidates[left].2[slot]
                        + candidates[right].2[slot]
                        + candidates[third].2[slot];
                }
                if Student::argmax(&scores) == sample.label {
                    return true;
                }
            }
        }
    }
    false
}

fn diagnose(student: &Student, samples: &[Sample], top_k: usize) -> Value {
    let wrong: Vec<&Sample> = samples
        .iter()
        .filter(|sample| Student::argmax(&student.scores(&sample.vector)) != sample.label)
        .collect();
    let (mut single, mut pairs, mut triples) = (0usize, 0usize, 0usize);
    for sample in &wrong {
        if single_reachable(student, sample) {
            single += 1;
            continue;
        }
        if pair_reachable(student, sample) {
            pairs += 1;
            continue;
        }
        if triple_reachable(student, sample, top_k) {
            triples += 1;
        }
    }
    let total = wrong.len();
    json!({
        "wrong": total,
        "single_fixable": single,
        "pair_only_fixable": pairs,
        "triple_only_fixable": triples,
        "unfixable": total - single - pairs - triples,
        "single_rate": if total == 0 { 1.0 } else { single as f64 / total as f64 },
        "single_or_pair_rate": if total == 0 { 1.0 } else { (single + pairs) as f64 / total as f64 },
        "single_pair_triple_rate": if total == 0 { 1.0 } else { (single + pairs + triples) as f64 / total as f64 },
        "top_k": top_k,
    })
}

fn main() {
    let args = parse_args();
    let payload: Value =
        serde_json::from_str(&std::fs::read_to_string(&args.samples).expect("samples"))
            .expect("json");
    let cols = payload.get("cols").and_then(Value::as_u64).expect("cols") as usize;
    let train = parse_samples(payload.get("train").expect("train"));
    let test = parse_samples(payload.get("test").expect("test"));
    let centroids = centroids(&train, cols);
    let layer = GroupedLayer::quantize(CLASSES, cols, &centroids, &[0.0; CLASSES], args.group)
        .expect("quantize");
    let student = Student::from_layer(&layer);

    let report = json!({
        "group": args.group,
        "train": diagnose(&student, &train, args.top_k),
        "test": diagnose(&student, &test, args.top_k),
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
