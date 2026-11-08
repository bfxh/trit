//! CIFAR-10（8×8 灰度）上的三值 MLP + 同架构 f32 基线 + 任务合并/调权读数。
//!
//! 数据夹具：`{split}_x8.bin`（每样本 64 字节灰度）+ `{split}_y.bin`（每样本 1 字节标签），
//! 由 `cifar32.npz` 降采样得到，不参与训练逻辑。
//!
//! ```text
//! cargo run --release -p trit-wspace --example cifar_tasks -- \
//!   --data <dir> [--binary] [--per-class 500] [--epochs 10] [--test-per-class 100] [--seeds 3]
//! ```
//!
//! `--binary` 取 cat(3) vs dog(5)，输出 1 维；默认是 10 类 one-hot。
//! 输入始终是三值（-1/0/+1）；f32 基线只解除**权重**三值化，数据和架构不变。

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use trit_learn::{Activation, Adam, Optimizer, Ste, TernaryDense, cross_entropy};
use trit_wspace::{
    Layer, Net, Oracle, TaskVector, Trit, merge_tasks_weighted, to_wsp, tune_task_weights,
    tune_task_weights_argmax, tune_task_weights_hamming, tune_task_weights_readout,
};

const IN: usize = 64;
const HIDDEN: usize = 64;
const ALL_CLASSES: usize = 10;

struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = (self.next_u64() as usize) % (i + 1);
            items.swap(i, j);
        }
    }
}

// ---------- 三值 MLP ----------

#[derive(Clone)]
struct Mlp {
    first: TernaryDense,
    second: TernaryDense,
}

impl Mlp {
    fn new(seed: u64, classes: usize) -> Self {
        let mut rng = Rng(seed);
        let first = TernaryDense::new(
            HIDDEN,
            IN,
            |_, _| (rng.f32() * 2.0 - 1.0) * 0.5,
            |_| 1.0 / (IN as f32).sqrt(),
            Ste::Identity,
        );
        let second = TernaryDense::new(
            classes,
            HIDDEN,
            |_, _| (rng.f32() * 2.0 - 1.0) * 0.5,
            |_| 1.0 / (HIDDEN as f32).sqrt(),
            Ste::Identity,
        );
        Self { first, second }
    }

    fn forward(&mut self, batch: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let hidden = self.first.forward(batch);
        let hidden = hidden
            .into_iter()
            .map(|v| Activation::Tanh.forward(v))
            .collect::<Vec<_>>();
        let logits = self.second.forward(&hidden);
        (logits, hidden)
    }
}

fn train_ternary(
    mlp: &mut Mlp,
    samples: &[(Vec<f32>, usize)],
    classes: usize,
    epochs: usize,
    lr: f32,
) -> f32 {
    let mut latent1 = Adam::new(lr);
    let mut scale1 = Adam::new(lr * 0.1);
    let mut latent2 = Adam::new(lr);
    let mut scale2 = Adam::new(lr * 0.1);
    let mut rng = Rng(0xC1FA_2026);
    let mut order: Vec<usize> = (0..samples.len()).collect();
    let mut loss_sum = 0.0f32;
    let mut steps = 0usize;
    for _ in 0..epochs {
        rng.shuffle(&mut order);
        for chunk in order.chunks(32) {
            let batch = chunk.len();
            let mut input = Vec::with_capacity(batch * IN);
            for &index in chunk {
                input.extend_from_slice(&samples[index].0);
            }
            let (logits, hidden) = mlp.forward(&input);
            let mut grad_logits = vec![0.0f32; batch * classes];
            for (t, &index) in chunk.iter().enumerate() {
                let (loss, grad) =
                    cross_entropy(&logits[t * classes..(t + 1) * classes], samples[index].1);
                loss_sum += loss;
                steps += 1;
                grad_logits[t * classes..(t + 1) * classes].copy_from_slice(&grad);
            }
            let grad_hidden = mlp.second.backward(&grad_logits);
            let mut grad_pre = vec![0.0f32; batch * HIDDEN];
            for t in 0..batch {
                for r in 0..HIDDEN {
                    let idx = t * HIDDEN + r;
                    grad_pre[idx] = grad_hidden[idx] * Activation::Tanh.backward(hidden[idx]);
                }
            }
            mlp.first.backward(&grad_pre);
            mlp.first.update(&mut latent1, &mut scale1);
            mlp.second.update(&mut latent2, &mut scale2);
            mlp.first.zero_grad();
            mlp.second.zero_grad();
        }
    }
    loss_sum / steps.max(1) as f32
}

fn to_net(mlp: &Mlp, output_rows: usize) -> Net {
    let first = Layer::with_scales(
        HIDDEN,
        IN,
        &mlp.first
            .quantized()
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
        &[0i8; HIDDEN],
        mlp.first.scales().to_vec(),
    )
    .expect("first layer");
    let mut trits = Vec::with_capacity(output_rows * HIDDEN);
    let mut scales = Vec::with_capacity(output_rows);
    for row in 0..output_rows {
        trits.extend_from_slice(&mlp.second.quantized()[row]);
        scales.push(mlp.second.scales()[row]);
    }
    let second = Layer::with_scales(output_rows, HIDDEN, &trits, &vec![0i8; output_rows], scales)
        .expect("second layer");
    Net::new(vec![first, second]).expect("net")
}

// ---------- 同架构 f32 基线 ----------

#[derive(Clone)]
struct DenseF32 {
    rows: usize,
    cols: usize,
    weight: Vec<f32>,
    bias: Vec<f32>,
    grad_w: Vec<f32>,
    grad_b: Vec<f32>,
    input: Vec<f32>,
    batch_rows: usize,
}

impl DenseF32 {
    fn new(rows: usize, cols: usize, mut init: impl FnMut(usize, usize) -> f32) -> Self {
        let mut weight = Vec::with_capacity(rows * cols);
        for r in 0..rows {
            for c in 0..cols {
                weight.push(init(r, c));
            }
        }
        Self {
            rows,
            cols,
            weight,
            bias: vec![0.0; rows],
            grad_w: vec![0.0; rows * cols],
            grad_b: vec![0.0; rows],
            input: Vec::new(),
            batch_rows: 0,
        }
    }

    fn forward(&mut self, batch: &[f32]) -> Vec<f32> {
        assert!(!batch.is_empty() && batch.len().is_multiple_of(self.cols));
        let rows_of_batch = batch.len() / self.cols;
        self.input.clear();
        self.input.extend_from_slice(batch);
        self.batch_rows = rows_of_batch;
        let mut out = vec![0.0f32; rows_of_batch * self.rows];
        for t in 0..rows_of_batch {
            for r in 0..self.rows {
                let mut sum = self.bias[r];
                for c in 0..self.cols {
                    sum += self.weight[r * self.cols + c] * batch[t * self.cols + c];
                }
                out[t * self.rows + r] = sum;
            }
        }
        out
    }

    fn backward(&mut self, grad_out: &[f32]) -> Vec<f32> {
        assert_eq!(grad_out.len(), self.batch_rows * self.rows);
        let mut grad_in = vec![0.0f32; self.batch_rows * self.cols];
        for t in 0..self.batch_rows {
            for r in 0..self.rows {
                let dy = grad_out[t * self.rows + r];
                self.grad_b[r] += dy;
                for c in 0..self.cols {
                    let x = self.input[t * self.cols + c];
                    self.grad_w[r * self.cols + c] += dy * x;
                    grad_in[t * self.cols + c] += dy * self.weight[r * self.cols + c];
                }
            }
        }
        grad_in
    }

    fn update(&mut self, opt_w: &mut Adam, opt_b: &mut Adam) {
        opt_w.step(&mut self.weight, &self.grad_w);
        opt_b.step(&mut self.bias, &self.grad_b);
    }

    fn zero_grad(&mut self) {
        self.grad_w.fill(0.0);
        self.grad_b.fill(0.0);
    }
}

#[derive(Clone)]
struct MlpF32 {
    first: DenseF32,
    second: DenseF32,
}

impl MlpF32 {
    fn new(seed: u64, classes: usize) -> Self {
        let mut rng = Rng(seed);
        let mut init = |_: usize, _: usize| (rng.f32() * 2.0 - 1.0) * 0.5;
        Self {
            first: DenseF32::new(HIDDEN, IN, &mut init),
            second: DenseF32::new(classes, HIDDEN, &mut init),
        }
    }

    fn forward(&mut self, batch: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let hidden = self.first.forward(batch);
        let hidden = hidden
            .into_iter()
            .map(|v| Activation::Tanh.forward(v))
            .collect::<Vec<_>>();
        let logits = self.second.forward(&hidden);
        (logits, hidden)
    }
}

fn train_f32(
    mlp: &mut MlpF32,
    samples: &[(Vec<f32>, usize)],
    classes: usize,
    epochs: usize,
    lr: f32,
) -> f32 {
    let mut opt_w1 = Adam::new(lr);
    let mut opt_b1 = Adam::new(lr);
    let mut opt_w2 = Adam::new(lr);
    let mut opt_b2 = Adam::new(lr);
    let mut rng = Rng(0xC1FA_2026);
    let mut order: Vec<usize> = (0..samples.len()).collect();
    let mut loss_sum = 0.0f32;
    let mut steps = 0usize;
    for _ in 0..epochs {
        rng.shuffle(&mut order);
        for chunk in order.chunks(32) {
            let batch = chunk.len();
            let mut input = Vec::with_capacity(batch * IN);
            for &index in chunk {
                input.extend_from_slice(&samples[index].0);
            }
            let (logits, hidden) = mlp.forward(&input);
            let mut grad_logits = vec![0.0f32; batch * classes];
            for (t, &index) in chunk.iter().enumerate() {
                let (loss, grad) =
                    cross_entropy(&logits[t * classes..(t + 1) * classes], samples[index].1);
                loss_sum += loss;
                steps += 1;
                grad_logits[t * classes..(t + 1) * classes].copy_from_slice(&grad);
            }
            let grad_hidden = mlp.second.backward(&grad_logits);
            let mut grad_pre = vec![0.0f32; batch * HIDDEN];
            for t in 0..batch {
                for r in 0..HIDDEN {
                    let idx = t * HIDDEN + r;
                    grad_pre[idx] = grad_hidden[idx] * Activation::Tanh.backward(hidden[idx]);
                }
            }
            mlp.first.backward(&grad_pre);
            mlp.first.update(&mut opt_w1, &mut opt_b1);
            mlp.second.update(&mut opt_w2, &mut opt_b2);
            mlp.first.zero_grad();
            mlp.second.zero_grad();
        }
    }
    loss_sum / steps.max(1) as f32
}

fn f32_accuracy(mlp: &mut MlpF32, inputs: &[Vec<f32>], labels: &[usize], binary: bool) -> f64 {
    let mut hit = 0usize;
    for (input, &label) in inputs.iter().zip(labels) {
        let (logits, _) = mlp.forward(input);
        let predicted = if binary {
            usize::from(logits[1] > logits[0])
        } else {
            let mut best = 0usize;
            let mut best_value = f32::NEG_INFINITY;
            for (index, value) in logits.iter().enumerate() {
                if *value > best_value {
                    best_value = *value;
                    best = index;
                }
            }
            best
        };
        if predicted == label {
            hit += 1;
        }
    }
    hit as f64 / inputs.len().max(1) as f64
}

// ---------- 数据 ----------

fn load(dir: &str, split: &str) -> (Vec<Vec<u8>>, Vec<usize>) {
    let raw = fs::read(Path::new(dir).join(format!("{split}_x8.bin"))).expect("read x8");
    let labels = fs::read(Path::new(dir).join(format!("{split}_y.bin"))).expect("read y");
    assert!(raw.len().is_multiple_of(IN));
    assert_eq!(raw.len() / IN, labels.len());
    let images = raw.chunks(IN).map(<[u8]>::to_vec).collect();
    (images, labels.into_iter().map(usize::from).collect())
}

fn quantize(image: &[u8]) -> Vec<f32> {
    image
        .iter()
        .map(|v| {
            if *v < 85 {
                -1.0
            } else if *v < 170 {
                0.0
            } else {
                1.0
            }
        })
        .collect()
}

fn take_per_class(
    images: &[Vec<u8>],
    labels: &[usize],
    per_class: usize,
    classes: &[usize],
) -> Vec<(Vec<f32>, usize)> {
    let mut counts = [0usize; ALL_CLASSES];
    let mut out = Vec::new();
    for (image, &label) in images.iter().zip(labels) {
        if !classes.contains(&label) || counts[label] >= per_class {
            continue;
        }
        counts[label] += 1;
        out.push((quantize(image), label));
    }
    out
}

fn take_binary(
    images: &[Vec<u8>],
    labels: &[usize],
    per_class: usize,
    first: usize,
    second: usize,
) -> Vec<(Vec<f32>, usize)> {
    let mut counts = [0usize; 2];
    let mut out = Vec::new();
    for (image, &label) in images.iter().zip(labels) {
        let target = if label == first {
            0
        } else if label == second {
            1
        } else {
            continue;
        };
        if counts[target] >= per_class {
            continue;
        }
        counts[target] += 1;
        out.push((quantize(image), target));
    }
    out
}

struct Dataset {
    mixed: Vec<(Vec<f32>, usize)>,
    left: Vec<(Vec<f32>, usize)>,
    right: Vec<(Vec<f32>, usize)>,
    test: Vec<(Vec<f32>, usize)>,
    classes: usize,
    output_rows: usize,
    binary: bool,
}

fn load_dataset(dir: &str, binary: bool, per_class: usize, test_per_class: usize) -> Dataset {
    let (train_images, train_labels) = load(dir, "train");
    let (test_images, test_labels) = load(dir, "test");
    let (mixed, left, right, test, classes, output_rows) = if binary {
        let all = take_binary(&train_images, &train_labels, per_class, 3, 5);
        let split = all.len() / 2;
        let left = all[..split].to_vec();
        let right = all[split..].to_vec();
        let test = take_binary(&test_images, &test_labels, test_per_class, 3, 5);
        (all, left, right, test, 2usize, 1usize)
    } else {
        let all_classes: Vec<usize> = (0..ALL_CLASSES).collect();
        let mixed = take_per_class(&train_images, &train_labels, per_class, &all_classes);
        let left = take_per_class(&train_images, &train_labels, per_class, &[0, 1, 2, 3, 4]);
        let right = take_per_class(&train_images, &train_labels, per_class, &[5, 6, 7, 8, 9]);
        let test = take_per_class(&test_images, &test_labels, test_per_class, &all_classes);
        (mixed, left, right, test, ALL_CLASSES, ALL_CLASSES)
    };
    Dataset {
        mixed,
        left,
        right,
        test,
        classes,
        output_rows,
        binary,
    }
}

fn one_hot(label: usize, classes: usize) -> Vec<Trit> {
    (0..classes)
        .map(|i| if i == label { Trit::Pos } else { Trit::Neg })
        .collect()
}

fn net_accuracy(net: &Net, inputs: &[Vec<Trit>], labels: &[usize], binary: bool) -> f64 {
    let mut hit = 0usize;
    for (input, &label) in inputs.iter().zip(labels) {
        let output = net.forward(input).expect("forward");
        let predicted = if binary {
            usize::from(output[0].to_i8() > 0)
        } else {
            let mut best = 0usize;
            let mut best_value = i8::MIN;
            for (index, trit) in output.iter().enumerate() {
                if trit.to_i8() > best_value {
                    best_value = trit.to_i8();
                    best = index;
                }
            }
            best
        };
        if predicted == label {
            hit += 1;
        }
    }
    hit as f64 / inputs.len().max(1) as f64
}

// ---------- 单次运行 ----------

fn run_ternary(seed: u64, data: &Dataset, epochs: usize) -> Value {
    let test_inputs: Vec<Vec<Trit>> = data
        .test
        .iter()
        .map(|(image, _)| {
            image
                .iter()
                .map(|v| {
                    if *v < 0.0 {
                        Trit::Neg
                    } else if *v > 0.0 {
                        Trit::Pos
                    } else {
                        Trit::Zero
                    }
                })
                .collect()
        })
        .collect();
    let test_labels: Vec<usize> = data.test.iter().map(|(_, label)| *label).collect();
    let oracle_outputs: Vec<Vec<Trit>> = test_labels
        .iter()
        .map(|&label| {
            if data.binary {
                vec![if label == 1 { Trit::Pos } else { Trit::Neg }]
            } else {
                one_hot(label, data.classes)
            }
        })
        .collect();
    let oracle = Oracle::new(test_inputs.clone(), oracle_outputs).expect("oracle");

    let mut base = Mlp::new(seed, data.classes);
    let base_loss = train_ternary(&mut base, &data.mixed, data.classes, epochs, 0.05);
    let mut task_a = base.clone();
    let a_loss = train_ternary(&mut task_a, &data.left, data.classes, epochs, 0.02);
    let mut task_b = base.clone();
    let b_loss = train_ternary(&mut task_b, &data.right, data.classes, epochs, 0.02);

    let base_net = to_net(&base, data.output_rows);
    let a_net = to_net(&task_a, data.output_rows);
    let b_net = to_net(&task_b, data.output_rows);
    let tasks = [
        TaskVector::between(&base_net, &a_net).expect("a vector"),
        TaskVector::between(&base_net, &b_net).expect("b vector"),
    ];
    let equal = merge_tasks_weighted(&base_net, &tasks, &[1.0, 1.0]).expect("equal merge");
    let exact = tune_task_weights(&base_net, &tasks, &oracle, 2).expect("exact");
    let hamming = tune_task_weights_hamming(&base_net, &tasks, &oracle, 2).expect("hamming");
    let argmax = tune_task_weights_argmax(&base_net, &tasks, &oracle, 2).expect("argmax");
    let readout = tune_task_weights_readout(&base_net, &tasks, &oracle, 2).expect("readout");
    let merged_exact =
        merge_tasks_weighted(&base_net, &tasks, exact.weights()).expect("exact merge");
    let merged_hamming =
        merge_tasks_weighted(&base_net, &tasks, hamming.weights()).expect("hamming merge");
    let merged_argmax =
        merge_tasks_weighted(&base_net, &tasks, argmax.weights()).expect("argmax merge");
    let merged_readout =
        merge_tasks_weighted(&base_net, &tasks, readout.weights()).expect("readout merge");

    let acc = |net: &Net| net_accuracy(net, &test_inputs, &test_labels, data.binary);
    json!({
        "base": acc(&base_net),
        "equal": acc(&equal),
        "exact": acc(&merged_exact),
        "hamming": acc(&merged_hamming),
        "argmax": acc(&merged_argmax),
        "readout": acc(&merged_readout),
        "train_loss": {
            "base": base_loss,
            "task_a": a_loss,
            "task_b": b_loss,
        },
        "tuning": {
            "exact": {"weights": exact.weights(), "loss": exact.loss()},
            "hamming": {"weights": hamming.weights(), "loss": hamming.loss()},
            "argmax": {"weights": argmax.weights(), "loss": argmax.loss()},
            "readout": {"weights": readout.weights(), "loss": readout.loss()},
        },
        "artifact": to_wsp(&merged_argmax),
    })
}

fn run_f32(seed: u64, data: &Dataset, epochs: usize) -> Value {
    let test_inputs: Vec<Vec<f32>> = data.test.iter().map(|(image, _)| image.clone()).collect();
    let test_labels: Vec<usize> = data.test.iter().map(|(_, label)| *label).collect();
    let mut base = MlpF32::new(seed, data.classes);
    let base_loss = train_f32(&mut base, &data.mixed, data.classes, epochs, 0.01);
    let mut task_a = base.clone();
    let a_loss = train_f32(&mut task_a, &data.left, data.classes, epochs, 0.005);
    let mut task_b = base.clone();
    let b_loss = train_f32(&mut task_b, &data.right, data.classes, epochs, 0.005);
    let acc = |mlp: &mut MlpF32| f32_accuracy(mlp, &test_inputs, &test_labels, data.binary);
    json!({
        "base": acc(&mut base),
        "task_a": acc(&mut task_a),
        "task_b": acc(&mut task_b),
        "train_loss": {
            "base": base_loss,
            "task_a": a_loss,
            "task_b": b_loss,
        },
    })
}

// ---------- 汇总 ----------

fn mean_std(values: &[f64]) -> Value {
    let n = values.len().max(1) as f64;
    let mean = values.iter().sum::<f64>() / n;
    let variance = if values.len() > 1 {
        values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0)
    } else {
        0.0
    };
    json!({"mean": mean, "std": variance.sqrt(), "values": values})
}

fn summarize_ternary(runs: &[Value]) -> Value {
    let column = |key: &str| -> Vec<f64> {
        runs.iter()
            .map(|run| run[key].as_f64().expect("accuracy"))
            .collect()
    };
    let mut out = serde_json::Map::new();
    for key in ["base", "equal", "exact", "hamming", "argmax", "readout"] {
        out.insert(key.to_string(), mean_std(&column(key)));
    }
    Value::Object(out)
}

fn summarize_f32(runs: &[Value]) -> Value {
    let column = |key: &str| -> Vec<f64> {
        runs.iter()
            .map(|run| run[key].as_f64().expect("accuracy"))
            .collect()
    };
    let mut out = serde_json::Map::new();
    for key in ["base", "task_a", "task_b"] {
        out.insert(key.to_string(), mean_std(&column(key)));
    }
    Value::Object(out)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut data_dir = None;
    let mut per_class = 500usize;
    let mut epochs = 10usize;
    let mut test_per_class = 100usize;
    let mut binary = false;
    let mut seeds = 1usize;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" => data_dir = args.next(),
            "--per-class" => per_class = args.next().expect("per-class").parse().expect("num"),
            "--epochs" => epochs = args.next().expect("epochs").parse().expect("num"),
            "--test-per-class" => {
                test_per_class = args.next().expect("test-per-class").parse().expect("num");
            }
            "--seeds" => seeds = args.next().expect("seeds").parse().expect("num"),
            "--binary" => binary = true,
            other => panic!("unknown arg {other}"),
        }
    }
    let data_dir = data_dir.expect("--data <dir>");
    let data = load_dataset(&data_dir, binary, per_class, test_per_class);
    let mut ternary = Vec::with_capacity(seeds);
    let mut f32_runs = Vec::with_capacity(seeds);
    for offset in 0..seeds {
        let seed = 7 + offset as u64;
        ternary.push(run_ternary(seed, &data, epochs));
        f32_runs.push(run_f32(seed, &data, epochs));
    }
    let report = json!({
        "mode": if binary { "binary-cat-vs-dog" } else { "10-class" },
        "seeds": seeds,
        "data": {
            "train": data.mixed.len(),
            "task_a": data.left.len(),
            "task_b": data.right.len(),
            "test": data.test.len(),
            "epochs": epochs,
        },
        "ternary": summarize_ternary(&ternary),
        "f32": summarize_f32(&f32_runs),
        "runs": {
            "ternary": ternary.iter().map(|run| json!({
                "base": run["base"], "equal": run["equal"], "argmax": run["argmax"],
                "tuning": run["tuning"],
            })).collect::<Vec<_>>(),
            "f32": f32_runs.iter().map(|run| json!({
                "base": run["base"], "task_a": run["task_a"], "task_b": run["task_b"],
            })).collect::<Vec<_>>(),
        },
        "artifact": ternary.first().map(|run| run["artifact"].clone()).unwrap_or(Value::Null),
    });
    println!("{}", serde_json::to_string_pretty(&report).expect("json"));
}
