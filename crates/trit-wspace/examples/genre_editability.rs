//! 可编辑性分层：genre 分类头上，错样本里有多少能被「改一个 trit」修好。
//!
//! 学生 = 类质心初始化的单层 `GroupedLayer`（3×384，与 `classifier_genre_repair` 相同）。
//! 这里是**单样本视角**：对每个错样本，枚举全部单 trit 改动（1152 位置 × 2 个候选值），
//! 看是否存在一个改动让它变对——不要求其他样本不变（那是全局编辑的事）。
//!
//! 输出：`wrong` / `single_trit_fixable` / `unfixable` 与若干修复样例。
//! 它回答的是「错样本的可编辑性上限」，不是「全局贪心能修多少」。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example genre_editability -- \
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
        limit: 16,
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

    fn predict(&self, input: &[f32]) -> usize {
        let mut best = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for row in 0..self.rows {
            let mut acc = 0.0f64;
            for (col, &value) in input.iter().enumerate() {
                acc += f64::from(self.weight(row, col)) * f64::from(value);
            }
            let score = acc as f32;
            if score > best_score {
                best_score = score;
                best = row;
            }
        }
        best
    }

    /// 该样本的单 trit 修复（只要求它自己变对）。
    fn single_fixes(&mut self, sample: &Sample) -> Vec<(usize, usize, i8)> {
        let mut fixes = Vec::new();
        for row in 0..self.rows {
            for col in 0..self.cols {
                let original = self.trits[row * self.cols + col];
                for candidate in [-1i8, 0, 1] {
                    if candidate == original {
                        continue;
                    }
                    self.trits[row * self.cols + col] = candidate;
                    let ok = self.predict(&sample.vector) == sample.label;
                    self.trits[row * self.cols + col] = original;
                    if ok {
                        fixes.push((row, col, candidate));
                    }
                }
            }
        }
        fixes
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

fn diagnose(student: &mut Student, samples: &[Sample], limit: usize) -> Value {
    let wrong: Vec<&Sample> = samples
        .iter()
        .filter(|sample| student.predict(&sample.vector) != sample.label)
        .collect();
    let mut fixable = 0usize;
    let mut examples = Vec::new();
    for sample in &wrong {
        let fixes = student.single_fixes(sample);
        if !fixes.is_empty() {
            fixable += 1;
        }
        if examples.len() < limit {
            examples.push(json!({
                "label": sample.label,
                "predicted": student.predict(&sample.vector),
                "fixes": fixes.iter().map(|(row, col, to)| json!({"row": row, "col": col, "to": to})).collect::<Vec<_>>(),
            }));
        }
    }
    let total = samples.len();
    let correct = total - wrong.len();
    json!({
        "samples": total,
        "correct": correct,
        "wrong": wrong.len(),
        "single_trit_fixable": fixable,
        "unfixable": wrong.len() - fixable,
        "fixable_rate": if wrong.is_empty() { 1.0 } else { fixable as f64 / wrong.len() as f64 },
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
    let mut student = Student::from_layer(&layer);

    let report = json!({
        "group": args.group,
        "train": diagnose(&mut student, &train, args.limit),
        "test": diagnose(&mut student, &test, args.limit),
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
