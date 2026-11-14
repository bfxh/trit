//! genre 三分类头上的推理式编辑：类质心初始化 + 行级探针 + 行内贪心。
//!
//! 任务比 STS pair 判定简单（3 类、1152 个 trit），用来回答「推理式编辑的上限」：
//! - 学生 = 单层 `GroupedLayer`（3×384，每类一行），输入是 f32 句向量，**argmax 判定**；
//! - 初始化 = 每类训练句向量的类质心，三值化 + 组 scale（无梯度）；
//! - 编辑 = 训练集上贪心搜「改一个 trit 能多对几条」，同时记录测试集准确率（看是否过拟合）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example classifier_genre_repair -- \
//!   --samples genre-samples.json --group 384 --max-edits 8
//! ```

use serde_json::{Value, json};
use trit_wspace::grouped::GroupedLayer;

const CLASSES: usize = 3;

struct Args {
    samples: String,
    group: usize,
    max_edits: usize,
    out: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args {
        samples: String::new(),
        group: 384,
        max_edits: 8,
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut next = || it.next().expect("缺少参数值");
        match arg.as_str() {
            "--samples" => args.samples = next(),
            "--group" => args.group = next().parse().expect("group"),
            "--max-edits" => args.max_edits = next().parse().expect("max-edits"),
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

    fn scores(&self, input: &[f32]) -> Vec<f32> {
        (0..self.rows)
            .map(|row| {
                let mut acc = 0.0f64;
                for (col, &value) in input.iter().enumerate() {
                    acc += f64::from(self.weight(row, col)) * f64::from(value);
                }
                acc as f32
            })
            .collect()
    }

    fn predict(&self, input: &[f32]) -> usize {
        let scores = self.scores(input);
        let mut best = 0usize;
        for index in 1..scores.len() {
            if scores[index] > scores[best] {
                best = index;
            }
        }
        best
    }

    fn correct(&self, samples: &[Sample]) -> usize {
        samples
            .iter()
            .filter(|sample| self.predict(&sample.vector) == sample.label)
            .count()
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

    let base_train = student.correct(&train);
    let base_test = student.correct(&test);
    let mut edits = Vec::new();
    let mut current_train = base_train;
    let mut current_test = base_test;

    for _ in 0..args.max_edits {
        let mut best: Option<(usize, usize, i8, usize, usize)> = None;
        // 行级探针 + 行内贪心合并成「逐行逐列试 trit」；行级消融只用于排序。
        let mut row_gain = [0isize; CLASSES];
        for (row, gain) in row_gain.iter_mut().enumerate() {
            let saved: Vec<i8> = (0..cols)
                .map(|col| student.trits[row * cols + col])
                .collect();
            for slot in student.trits[row * cols..(row + 1) * cols].iter_mut() {
                *slot = 0;
            }
            let zeroed = student.correct(&train) as isize - current_train as isize;
            for (col, &value) in saved.iter().enumerate() {
                student.trits[row * cols + col] = value;
            }
            *gain = zeroed;
        }
        let mut order: Vec<usize> = (0..CLASSES).collect();
        order.sort_by_key(|&row| std::cmp::Reverse(row_gain[row]));

        for row in order {
            for col in 0..cols {
                let original = student.trits[row * cols + col];
                for candidate in [-1i8, 0, 1] {
                    if candidate == original {
                        continue;
                    }
                    student.trits[row * cols + col] = candidate;
                    let train_correct = student.correct(&train);
                    let test_correct = student.correct(&test);
                    student.trits[row * cols + col] = original;
                    if train_correct > current_train
                        && best.is_none_or(|(_, _, _, best_train, _)| train_correct > best_train)
                    {
                        best = Some((row, col, candidate, train_correct, test_correct));
                    }
                }
            }
        }
        let Some((row, col, to, train_correct, test_correct)) = best else {
            break;
        };
        student.trits[row * cols + col] = to;
        current_train = train_correct;
        current_test = test_correct;
        edits.push(json!({
            "row": row, "col": col, "to": to,
            "train_correct": train_correct,
            "test_correct": test_correct,
            "train_gain": train_correct as isize - base_train as isize,
        }));
    }

    let report = json!({
        "samples": {"train": train.len(), "test": test.len()},
        "classes": CLASSES,
        "group": args.group,
        "baseline": {
            "train_correct": base_train,
            "train_accuracy": base_train as f64 / train.len() as f64,
            "test_correct": base_test,
            "test_accuracy": base_test as f64 / test.len() as f64,
        },
        "repair": {
            "steps": edits.len(),
            "edits": edits,
            "train_correct": current_train,
            "train_accuracy": current_train as f64 / train.len() as f64,
            "test_correct": current_test,
            "test_accuracy": current_test as f64 / test.len() as f64,
            "train_gain": current_train as isize - base_train as isize,
            "test_gain": current_test as isize - base_test as isize,
        },
    });
    let text = serde_json::to_string_pretty(&report).expect("json");
    if let Some(path) = &args.out {
        std::fs::write(path, &text).expect("write report");
    }
    println!("{text}");
}
