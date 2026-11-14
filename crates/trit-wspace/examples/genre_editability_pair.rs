//! 双 trit 组合的可编辑性上界：单 trit 修不了的错样本，两个 trit 能修多少。
//!
//! 增量算法：先算该样本的 3 行基础分数，再为每个单候选（位置 × 目标值）预计算它造成的
//! 行分数增量——双组合只是两次增量相加，`O(1)` 判断 argmax，不需要重跑完整前向。
//! 只对「单 trit 不可修」的错样本做双组合，否则组合数会平方爆炸。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example genre_editability_pair -- \
//!   --samples genre-samples.json --group 384
//! ```

use serde_json::{Value, json};
use trit_wspace::grouped::GroupedLayer;

const CLASSES: usize = 3;

struct Args {
    samples: String,
    group: usize,
    limit: usize,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        samples: String::new(),
        group: 384,
        limit: 8,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--samples" => args.samples = next(),
            "--group" => args.group = next().parse().expect("group"),
            "--limit" => args.limit = next().parse().expect("limit"),
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

/// 双 trit：返回第一个可达组合（若存在）。
fn pair_reachable(student: &Student, sample: &Sample) -> Option<(usize, i8, usize, i8)> {
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
                return Some((
                    candidates[left].0,
                    candidates[left].1,
                    candidates[right].0,
                    candidates[right].1,
                ));
            }
        }
    }
    None
}

fn diagnose(student: &Student, samples: &[Sample], limit: usize) -> Value {
    let wrong: Vec<&Sample> = samples
        .iter()
        .filter(|sample| Student::argmax(&student.scores(&sample.vector)) != sample.label)
        .collect();
    let mut single = 0usize;
    let mut pairs = 0usize;
    let mut examples = Vec::new();
    for sample in &wrong {
        if single_reachable(student, sample) {
            single += 1;
            continue;
        }
        let pair = pair_reachable(student, sample);
        if pair.is_some() {
            pairs += 1;
        }
        if examples.len() < limit {
            examples.push(json!({
                "label": sample.label,
                "pair_fix": pair.map(|(left_pos, left_to, right_pos, right_to)| json!({
                    "left": {"row": left_pos / student.cols, "col": left_pos % student.cols, "to": left_to},
                    "right": {"row": right_pos / student.cols, "col": right_pos % student.cols, "to": right_to},
                })),
            }));
        }
    }
    let total = wrong.len();
    json!({
        "wrong": total,
        "single_fixable": single,
        "pair_only_fixable": pairs,
        "unfixable": total - single - pairs,
        "single_rate": if total == 0 { 1.0 } else { single as f64 / total as f64 },
        "single_or_pair_rate": if total == 0 { 1.0 } else { (single + pairs) as f64 / total as f64 },
        "examples": examples,
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
        "train": diagnose(&student, &train, args.limit),
        "test": diagnose(&student, &test, args.limit),
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
