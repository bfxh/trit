//! CIFAR-10（8×8 灰度）上的三值 MLP 训练 + 任务合并/调权读数。
//!
//! 数据夹具：`{split}_x8.bin`（每样本 64 字节灰度）+ `{split}_y.bin`（每样本 1 字节标签），
//! 由 `cifar32.npz` 降采样得到，不参与训练逻辑。
//!
//! ```text
//! cargo run --release -p trit-wspace --example cifar_tasks -- \
//!   --data <dir> [--binary] [--per-class 500] [--epochs 10] [--test-per-class 100]
//! ```
//!
//! `--binary` 取 cat(3) vs dog(5)，输出 1 维；默认是 10 类 one-hot。

use std::fs;
use std::path::Path;

use serde_json::json;
use trit_learn::{Activation, Adam, Ste, TernaryDense, cross_entropy};
use trit_wspace::{
    Layer, Net, Oracle, TaskVector, Trit, merge_tasks_weighted, to_wsp, tune_task_weights,
    tune_task_weights_hamming,
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

    fn forward(&mut self, batch: &[f32], classes: usize) -> (Vec<f32>, Vec<f32>) {
        let hidden = self.first.forward(batch);
        let hidden = hidden
            .into_iter()
            .map(|v| Activation::Tanh.forward(v))
            .collect::<Vec<_>>();
        let logits = self.second.forward(&hidden);
        debug_assert_eq!(logits.len() % classes, 0);
        (logits, hidden)
    }
}

fn train(
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
            let (logits, hidden) = mlp.forward(&input, classes);
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

fn trits_of(dense: &TernaryDense, row: usize) -> Vec<i8> {
    dense.quantized()[row].clone()
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
        trits.extend_from_slice(&trits_of(&mlp.second, row));
        scales.push(mlp.second.scales()[row]);
    }
    let second = Layer::with_scales(output_rows, HIDDEN, &trits, &vec![0i8; output_rows], scales)
        .expect("second layer");
    Net::new(vec![first, second]).expect("net")
}

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

fn one_hot(label: usize, classes: usize) -> Vec<Trit> {
    (0..classes)
        .map(|i| if i == label { Trit::Pos } else { Trit::Neg })
        .collect()
}

fn argmax_accuracy(net: &Net, inputs: &[Vec<Trit>], labels: &[usize], binary: bool) -> f64 {
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

fn report(
    net: &Net,
    inputs: &[Vec<Trit>],
    labels: &[usize],
    oracle: &Oracle,
    binary: bool,
) -> serde_json::Value {
    json!({
        "argmax": argmax_accuracy(net, inputs, labels, binary),
        "oracle_exact": 1.0 - oracle.loss(net).expect("loss") as f64 / inputs.len().max(1) as f64,
    })
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut data = None;
    let mut per_class = 500usize;
    let mut epochs = 10usize;
    let mut test_per_class = 100usize;
    let mut binary = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" => data = args.next(),
            "--per-class" => per_class = args.next().expect("per-class").parse().expect("num"),
            "--epochs" => epochs = args.next().expect("epochs").parse().expect("num"),
            "--test-per-class" => {
                test_per_class = args.next().expect("test-per-class").parse().expect("num");
            }
            "--binary" => binary = true,
            other => panic!("unknown arg {other}"),
        }
    }
    let data = data.expect("--data <dir>");
    let (train_images, train_labels) = load(&data, "train");
    let (test_images, test_labels) = load(&data, "test");

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

    let test_inputs: Vec<Vec<Trit>> = test
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
    let test_labels: Vec<usize> = test.iter().map(|(_, label)| *label).collect();
    let oracle_outputs: Vec<Vec<Trit>> = test_labels
        .iter()
        .map(|&label| {
            if binary {
                vec![if label == 1 { Trit::Pos } else { Trit::Neg }]
            } else {
                one_hot(label, classes)
            }
        })
        .collect();
    let oracle = Oracle::new(test_inputs.clone(), oracle_outputs).expect("oracle");

    let mut base = Mlp::new(7, classes);
    let base_loss = train(&mut base, &mixed, classes, epochs, 0.05);
    let mut task_a = base.clone();
    let a_loss = train(&mut task_a, &left, classes, epochs, 0.02);
    let mut task_b = base.clone();
    let b_loss = train(&mut task_b, &right, classes, epochs, 0.02);

    let base_net = to_net(&base, output_rows);
    let a_net = to_net(&task_a, output_rows);
    let b_net = to_net(&task_b, output_rows);
    let tasks = [
        TaskVector::between(&base_net, &a_net).expect("a vector"),
        TaskVector::between(&base_net, &b_net).expect("b vector"),
    ];
    let equal = merge_tasks_weighted(&base_net, &tasks, &[1.0, 1.0]).expect("equal merge");
    let tuning_exact = tune_task_weights(&base_net, &tasks, &oracle, 2).expect("tune exact");
    let tuned_exact =
        merge_tasks_weighted(&base_net, &tasks, tuning_exact.weights()).expect("tuned exact");
    let tuning_hamming =
        tune_task_weights_hamming(&base_net, &tasks, &oracle, 2).expect("tune hamming");
    let tuned_hamming =
        merge_tasks_weighted(&base_net, &tasks, tuning_hamming.weights()).expect("tuned hamming");

    let report_value = json!({
        "mode": if binary { "binary-cat-vs-dog" } else { "10-class" },
        "data": {
            "train": mixed.len(),
            "task_a": left.len(),
            "task_b": right.len(),
            "test": test.len(),
            "epochs": epochs,
        },
        "train_loss": {
            "base": base_loss,
            "task_a": a_loss,
            "task_b": b_loss,
        },
        "base": report(&base_net, &test_inputs, &test_labels, &oracle, binary),
        "task_a": report(&a_net, &test_inputs, &test_labels, &oracle, binary),
        "task_b": report(&b_net, &test_inputs, &test_labels, &oracle, binary),
        "equal_merge": report(&equal, &test_inputs, &test_labels, &oracle, binary),
        "tuned_exact": report(&tuned_exact, &test_inputs, &test_labels, &oracle, binary),
        "tuned_hamming": report(&tuned_hamming, &test_inputs, &test_labels, &oracle, binary),
        "tuning": {
            "exact": {
                "weights": tuning_exact.weights(),
                "initial_loss": tuning_exact.initial_loss(),
                "tuned_loss": tuning_exact.loss(),
                "evaluations": tuning_exact.evaluations(),
            },
            "hamming": {
                "weights": tuning_hamming.weights(),
                "initial_loss": tuning_hamming.initial_loss(),
                "tuned_loss": tuning_hamming.loss(),
                "evaluations": tuning_hamming.evaluations(),
            },
        },
        "artifact": to_wsp(&tuned_hamming),
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report_value).expect("json")
    );
}
