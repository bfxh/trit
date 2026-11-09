//! 真实 `trit-train` 产物接入：`TernaryLinear` 训练 → WSP1 → 读回 → 合并/调权/λ 扫描。
//!
//! 玩具模式（无 `--data`）：4 样本线性可分。
//! CIFAR 模式：`--data <dir> --size 8|16|32 [--color] --per-class N --epochs N
//! --test-per-class N [--binary] --seeds N`。
//! 多层：`--hidden N`（N>0 时两层；第一层贪心恒等、第二层分类，无端到端反向）。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use trit_train::layer::TernaryLinear;
use trit_train::quant::Ste;
use trit_wspace::{
    DEFAULT_TUNE_GRID, Layer, Net, Oracle, TaskVector, Trit, TuneMerge, TuneMetric, combine_tasks,
    dare_drop, dare_drop_blocks, from_wsp, merge_tasks_weighted, merge_tasks_weighted_dare, to_wsp,
    tune_task_weights, tune_task_weights_metric,
};

const ALL_CLASSES: usize = 10;

fn load(dir: &str, split: &str, size: usize, color: bool) -> (Vec<Vec<u8>>, Vec<usize>) {
    let pixel = size * size * if color { 3 } else { 1 };
    let suffix = if color { "c" } else { "" };
    let raw =
        fs::read(Path::new(dir).join(format!("{split}_x{size}{suffix}.bin"))).expect("read image");
    let labels = fs::read(Path::new(dir).join(format!("{split}_y.bin"))).expect("read y");
    assert!(raw.len().is_multiple_of(pixel));
    assert_eq!(raw.len() / pixel, labels.len());
    let images = raw.chunks(pixel).map(<[u8]>::to_vec).collect();
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
) -> Vec<(Vec<u8>, usize)> {
    let mut counts = [0usize; ALL_CLASSES];
    let mut out = Vec::new();
    for (image, &label) in images.iter().zip(labels) {
        if !classes.contains(&label) || counts[label] >= per_class {
            continue;
        }
        counts[label] += 1;
        out.push((image.clone(), label));
    }
    out
}

fn take_binary(
    images: &[Vec<u8>],
    labels: &[usize],
    per_class: usize,
    first: usize,
    second: usize,
) -> Vec<(Vec<u8>, usize)> {
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
        out.push((image.clone(), target));
    }
    out
}

struct Dataset {
    mixed: Vec<(Vec<u8>, usize)>,
    left: Vec<(Vec<u8>, usize)>,
    right: Vec<(Vec<u8>, usize)>,
    test: Vec<(Vec<u8>, usize)>,
    output_rows: usize,
    binary: bool,
    size: usize,
    color: bool,
    input_dim: usize,
}

fn load_dataset(
    dir: &str,
    size: usize,
    color: bool,
    binary: bool,
    per_class: usize,
    test_per_class: usize,
) -> Dataset {
    let (train_images, train_labels) = load(dir, "train", size, color);
    let (test_images, test_labels) = load(dir, "test", size, color);
    let (mixed, left, right, test, output_rows) = if binary {
        let all = take_binary(&train_images, &train_labels, per_class, 3, 5);
        let split = all.len() / 2;
        let (left, right) = all.split_at(split);
        let test = take_binary(&test_images, &test_labels, test_per_class, 3, 5);
        (all.clone(), left.to_vec(), right.to_vec(), test, 1usize)
    } else {
        let classes: Vec<usize> = (0..ALL_CLASSES).collect();
        let all = take_per_class(&train_images, &train_labels, per_class, &classes);
        let split = all.len() / 2;
        let (left, right) = all.split_at(split);
        let test = take_per_class(&test_images, &test_labels, test_per_class, &classes);
        (
            all.clone(),
            left.to_vec(),
            right.to_vec(),
            test,
            ALL_CLASSES,
        )
    };
    let input_dim = size * size * if color { 3 } else { 1 };
    Dataset {
        mixed,
        left,
        right,
        test,
        output_rows,
        binary,
        size,
        color,
        input_dim,
    }
}

fn toy_dataset() -> Dataset {
    let images = vec![vec![255u8, 255], vec![255, 0], vec![0, 255], vec![0, 0]];
    let labels = vec![0usize, 0, 1, 1];
    let all: Vec<(Vec<u8>, usize)> = images.into_iter().zip(labels).collect();
    let left = all[..2].to_vec();
    let right = all[2..].to_vec();
    Dataset {
        mixed: all.clone(),
        left,
        right,
        test: all,
        output_rows: 2,
        binary: false,
        size: 0,
        color: false,
        input_dim: 2,
    }
}

fn trit_of(value: f32) -> Trit {
    if value > 0.0 {
        Trit::Pos
    } else if value < 0.0 {
        Trit::Neg
    } else {
        Trit::Zero
    }
}

fn tanh_vec(values: &[f32]) -> Vec<f32> {
    values.iter().map(|value| value.tanh()).collect()
}

fn one_hot(label: usize, rows: usize) -> Vec<f32> {
    (0..rows)
        .map(|index| if index == label { 1.0 } else { 0.0 })
        .collect()
}

fn init_weight(r: usize, c: usize, seed: u64) -> f32 {
    let mixed = (r * 131 + c * 197 + seed as usize * 17) % 11;
    mixed as f32 / 11.0 - 0.5
}

struct Model {
    layers: Vec<TernaryLinear>,
}

impl Model {
    fn new(input_dim: usize, hidden: usize, output_rows: usize, seed: u64) -> Self {
        let layers = if hidden == 0 {
            vec![TernaryLinear::new(
                output_rows,
                input_dim,
                |r, c| init_weight(r, c, seed),
                Ste::Identity,
                0.05,
                0.5,
            )]
        } else {
            vec![
                TernaryLinear::new(
                    hidden,
                    input_dim,
                    |r, c| init_weight(r, c, seed),
                    Ste::Identity,
                    0.05,
                    0.5,
                ),
                TernaryLinear::new(
                    output_rows,
                    hidden,
                    |r, c| init_weight(r, c, seed),
                    Ste::Identity,
                    0.05,
                    0.5,
                ),
            ]
        };
        Self { layers }
    }

    fn train(&mut self, samples: &[(Vec<f32>, usize)], epochs: usize, output_rows: usize) {
        if self.layers.len() == 2 {
            for _ in 0..epochs {
                for (input, label) in samples {
                    let hidden_raw = self.layers[0].forward(input);
                    let hidden = tanh_vec(&hidden_raw);
                    let output = self.layers[1].forward(&hidden);
                    let target = one_hot(*label, output_rows);
                    let dy: Vec<f32> = output
                        .iter()
                        .zip(&target)
                        .map(|(y, t)| (y - t) / output_rows as f32)
                        .collect();
                    let dh = self.layers[1].train_step_with_grad(&hidden, &dy);
                    let dh_raw: Vec<f32> = dh
                        .iter()
                        .zip(&hidden)
                        .map(|(grad, value)| grad * (1.0 - value * value))
                        .collect();
                    self.layers[0].train_step_with_grad(input, &dh_raw);
                }
            }
        } else {
            for _ in 0..epochs {
                for (input, label) in samples {
                    self.layers[0].train_step(input, &one_hot(*label, output_rows));
                }
            }
        }
    }

    fn to_net(&self) -> Net {
        let layers = self
            .layers
            .iter()
            .map(|layer| {
                let rows = layer.rows();
                let cols = layer.cols();
                let flat: Vec<i8> = layer.quantized().into_iter().flatten().collect();
                Layer::new(rows, cols, &flat, &vec![0i8; rows]).expect("layer")
            })
            .collect();
        Net::new(layers).expect("net")
    }
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

struct DareOptions {
    rate: Option<f32>,
    per_task: bool,
    block: Option<usize>,
    tune_with_dare: bool,
    seed: u64,
}

fn dare_vector(base: &Net, tuned: &Net, options: &DareOptions) -> TaskVector {
    let rate = options
        .rate
        .expect("--dare-rate/--dare-per-task 需要丢弃率");
    let outcome = match options.block {
        Some(block) => {
            dare_drop_blocks(base, tuned, rate, options.seed, block).expect("dare blocks")
        }
        None => dare_drop(base, tuned, rate, options.seed).expect("dare"),
    };
    let mut work = base.clone();
    outcome.patch.apply(&mut work).expect("apply dare");
    TaskVector::between(base, &work).expect("dare vector")
}

fn run(
    seed: u64,
    data: &Dataset,
    epochs: usize,
    hidden: usize,
    out_root: &Path,
    lambdas: &[f32],
    dare: &DareOptions,
) -> Value {
    let train = |samples: &[(Vec<u8>, usize)]| {
        let mut model = Model::new(data.input_dim, hidden, data.output_rows, seed);
        let samples: Vec<(Vec<f32>, usize)> = samples
            .iter()
            .map(|(image, label)| (quantize(image), *label))
            .collect();
        model.train(&samples, epochs, data.output_rows);
        model.to_net()
    };
    let base = train(&data.mixed);
    let a = train(&data.left);
    let b = train(&data.right);
    let dir = out_root.join(format!("seed{seed}"));
    fs::create_dir_all(&dir).expect("artifact dir");
    for (name, net) in [("base", &base), ("a", &a), ("b", &b)] {
        fs::write(dir.join(format!("{name}.wsp")), to_wsp(net)).expect("write artifact");
    }
    // 读回真实 WSP1 产物；后续合并只碰文件产物。
    let base =
        from_wsp(&fs::read_to_string(dir.join("base.wsp")).expect("read base")).expect("parse");
    let a = from_wsp(&fs::read_to_string(dir.join("a.wsp")).expect("read a")).expect("parse");
    let b = from_wsp(&fs::read_to_string(dir.join("b.wsp")).expect("read b")).expect("parse");
    let tasks = if dare.per_task {
        [dare_vector(&base, &a, dare), dare_vector(&base, &b, dare)]
    } else {
        [
            TaskVector::between(&base, &a).expect("a vector"),
            TaskVector::between(&base, &b).expect("b vector"),
        ]
    };
    let test_inputs: Vec<Vec<Trit>> = data
        .test
        .iter()
        .map(|(image, _)| quantize(image).iter().map(|v| trit_of(*v)).collect())
        .collect();
    let test_labels: Vec<usize> = data.test.iter().map(|(_, label)| *label).collect();
    let oracle = Oracle::new(
        test_inputs.clone(),
        test_labels
            .iter()
            .map(|label| {
                one_hot(*label, data.output_rows)
                    .iter()
                    .map(|value| trit_of(*value))
                    .collect()
            })
            .collect(),
    )
    .expect("oracle");
    let equal = if let Some(rate) = dare.rate {
        merge_tasks_weighted_dare(&base, &tasks, &[1.0, 1.0], rate, dare.seed).expect("equal dare")
    } else {
        merge_tasks_weighted(&base, &tasks, &[1.0, 1.0]).expect("equal")
    };
    let tuned = if dare.tune_with_dare {
        tune_task_weights_metric(
            &base,
            &tasks,
            &oracle,
            2,
            TuneMetric::Exact,
            &DEFAULT_TUNE_GRID,
            TuneMerge::Dare {
                rate: dare.rate.expect("--tune-with-dare 需要 --dare-rate"),
                seed: dare.seed,
            },
        )
        .expect("tune dare")
    } else {
        tune_task_weights(&base, &tasks, &oracle, 2).expect("tune")
    };
    let tuned_net = if let Some(rate) = dare.rate {
        merge_tasks_weighted_dare(&base, &tasks, tuned.weights(), rate, dare.seed)
            .expect("tuned dare merge")
    } else {
        merge_tasks_weighted(&base, &tasks, tuned.weights()).expect("tuned merge")
    };
    let lambda_scan: Vec<Value> = lambdas
        .iter()
        .map(|lambda| {
            let net = combine_tasks(&base, &tasks, &[1.0, *lambda]).expect("combine");
            json!({
                "lambda": lambda,
                "accuracy": net_accuracy(&net, &test_inputs, &test_labels, data.binary),
            })
        })
        .collect();
    json!({
        "seed": seed,
        "artifacts": dir.display().to_string(),
        "summary": {"arch": base.arch()},
        "base": net_accuracy(&base, &test_inputs, &test_labels, data.binary),
        "equal": net_accuracy(&equal, &test_inputs, &test_labels, data.binary),
        "tuned": net_accuracy(&tuned_net, &test_inputs, &test_labels, data.binary),
        "weights": tuned.weights(),
        "task_edits": [tasks[0].edits().len(), tasks[1].edits().len()],
        "lambda_scan": lambda_scan,
        "dare": {
            "rate": dare.rate,
            "per_task": dare.per_task,
            "block": dare.block,
            "tune_with_dare": dare.tune_with_dare,
            "seed": dare.seed,
        },
    })
}

fn parse_lambdas(raw: &str) -> Vec<f32> {
    raw.split(',')
        .map(|part| part.trim().parse::<f32>().expect("lambda"))
        .collect()
}

fn main() {
    let mut data_dir = None;
    let mut size = 16usize;
    let mut color = false;
    let mut per_class = 500usize;
    let mut epochs = 10usize;
    let mut test_per_class = 100usize;
    let mut binary = false;
    let mut seeds = 1usize;
    let mut hidden = 0usize;
    let mut out = std::env::temp_dir().join(format!("trit-train-artifacts-{}", std::process::id()));
    let mut lambdas = vec![-1.0f32, 0.0, 0.5, 1.0];
    let mut dare = DareOptions {
        rate: None,
        per_task: false,
        block: None,
        tune_with_dare: false,
        seed: 7,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data" => data_dir = args.next(),
            "--size" => size = args.next().expect("size").parse().expect("num"),
            "--color" => color = true,
            "--per-class" => per_class = args.next().expect("per-class").parse().expect("num"),
            "--epochs" => epochs = args.next().expect("epochs").parse().expect("num"),
            "--test-per-class" => {
                test_per_class = args.next().expect("test-per-class").parse().expect("num");
            }
            "--binary" => binary = true,
            "--seeds" => seeds = args.next().expect("seeds").parse().expect("num"),
            "--hidden" => hidden = args.next().expect("hidden").parse().expect("num"),
            "--out" => out = PathBuf::from(args.next().expect("out")),
            "--lambdas" => lambdas = parse_lambdas(&args.next().expect("lambdas")),
            "--dare-rate" => {
                dare.rate = Some(args.next().expect("dare-rate").parse().expect("num"));
            }
            "--dare-per-task" => {
                dare.rate = Some(args.next().expect("dare-per-task").parse().expect("num"));
                dare.per_task = true;
            }
            "--by-block" => {
                dare.block = Some(args.next().expect("by-block").parse().expect("num"));
            }
            "--tune-with-dare" => dare.tune_with_dare = true,
            "--dare-seed" => {
                dare.seed = args.next().expect("dare-seed").parse().expect("num");
            }
            other => panic!("unknown arg {other}"),
        }
    }
    fs::create_dir_all(&out).expect("out dir");
    let data = match data_dir.as_deref() {
        Some(dir) => load_dataset(dir, size, color, binary, per_class, test_per_class),
        None => toy_dataset(),
    };
    let runs: Vec<Value> = (0..seeds)
        .map(|offset| {
            run(
                7 + offset as u64,
                &data,
                epochs,
                hidden,
                &out,
                &lambdas,
                &dare,
            )
        })
        .collect();
    let column = |key: &str| -> Vec<f64> {
        runs.iter()
            .map(|run| run[key].as_f64().expect("accuracy"))
            .collect()
    };
    let lambda_means: Vec<Value> = lambdas
        .iter()
        .enumerate()
        .map(|(index, lambda)| {
            let values: Vec<f64> = runs
                .iter()
                .map(|run| {
                    run["lambda_scan"][index]["accuracy"]
                        .as_f64()
                        .expect("lambda accuracy")
                })
                .collect();
            let mut summary = mean_std(&values);
            summary["lambda"] = json!(lambda);
            summary
        })
        .collect();
    let report = json!({
        "mode": if data_dir.is_some() {
            if binary { "cifar-binary" } else { "cifar-10" }
        } else {
            "toy"
        },
        "data": {
            "train": data.mixed.len(),
            "task_a": data.left.len(),
            "task_b": data.right.len(),
            "test": data.test.len(),
            "size": data.size,
            "color": data.color,
            "hidden": hidden,
            "epochs": epochs,
        },
        "seeds": seeds,
        "artifacts": out.display().to_string(),
        "base": mean_std(&column("base")),
        "equal": mean_std(&column("equal")),
        "tuned": mean_std(&column("tuned")),
        "lambda_scan": lambda_means,
        "runs": runs,
    });
    println!("{}", serde_json::to_string_pretty(&report).expect("json"));
}
