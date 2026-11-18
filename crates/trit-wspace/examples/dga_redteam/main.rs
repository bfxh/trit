//! 三值 QAT 训练 + 工件红队：DGA 域名检测与家族归因（26 类，真实数据集）。
//!
//! 数据：`chrmor/DGA_domains_dataset`（Netlab 360 的 25 个 DGA 家族 + Alexa
//! benign，67.5 万条，`label,family,domain` CSV，研究许可）——**真实世界数据，
//! 白客防御任务**：不是「是不是恶意」，还包括「是哪个家族」（归因 = 应急响应
//! 的输入）。每类下采样 800（20.8k），全程纯 Rust（训练禁 Python 硬门一致）。
//!
//! 部件：特征 = 域名 2/3-gram FNV 哈希 2048 维（出现=+1，输入三值）；模型 =
//! QAT `TernaryDense` 2048→64→26（`Ste::Clipped`，Tanh 隐层，softmax 交叉熵）；
//! 评测 = `trit_learn::metrics::Confusion`（acc / macro-F1 / per-class）；
//! 红队 = 比特翻转攻击（`Edit::SetTrit`）后的归因准确率与家族混淆变化。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example dga_redteam -- \
//!   --file <数据盘>/trit-data/dga/dga_domains_full.csv --per-class 800
//! ```

mod features;

use std::collections::BTreeMap;
use std::fs;

use serde_json::{Value, json};
use trit_learn::dense::TernaryDense;
use trit_learn::loss::cross_entropy;
use trit_learn::metrics::Confusion;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

use features::featurize;

/// xorshift64*（同 seed 同划分/同攻击）。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() % 2001) as f32 / 1000.0 - 1.0
    }
}

/// CSV 一行 `label,family,domain`；`legit` → 0，每个 DGA 家族 → 1..26。
fn load_dataset(path: &str, binary: bool) -> (Vec<usize>, Vec<String>, BTreeMap<usize, String>) {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("读 {path}: {error}"));
    let mut family_ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut labels = Vec::new();
    let mut domains = Vec::new();
    for line in text.lines().skip(1) {
        let parts: Vec<&str> = line.splitn(3, ',').collect();
        if parts.len() < 3 {
            continue;
        }
        let label = match parts[0] {
            "legit" => 0,
            "dga" => {
                if binary {
                    1
                } else {
                    let family = parts[1].to_string();
                    let next = family_ids.len() + 1;
                    *family_ids.entry(family).or_insert(next)
                }
            }
            other => panic!("未知标签 {other}"),
        };
        labels.push(label);
        domains.push(parts[2].to_string());
    }
    let names: BTreeMap<usize, String> = family_ids
        .into_iter()
        .map(|(name, id)| (id, name))
        .collect();
    (labels, domains, names)
}

/// 每类下采样到 `per_class` 个（确定性保留，seed 由调用方洗牌后截断）。
fn downsample(labels: &[usize], per_class: usize, rng: &mut Rng) -> Vec<usize> {
    let mut by_class: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, &label) in labels.iter().enumerate() {
        by_class.entry(label).or_default().push(index);
    }
    let mut keep = Vec::new();
    for indices in by_class.values_mut() {
        let n = indices.len();
        for i in (1..n).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            indices.swap(i, j);
        }
        keep.extend(&indices[..per_class.min(n)]);
    }
    keep.sort_unstable();
    keep
}

/// 多分类 QAT 推理：隐层 Tanh → 输出 logits（argmax 由调用方做）。
fn forward_logits(
    hidden: &mut TernaryDense,
    out_layer: &mut TernaryDense,
    features: &[Vec<f32>],
    i: usize,
) -> Vec<f32> {
    let h = hidden.forward(&features[i]);
    let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
    out_layer.forward(&h_act)
}

fn main() {
    let mut file = String::from("dga_domains_full.csv");
    let mut per_class = 800usize;
    let mut epochs = 24usize;
    let mut binary = false;
    let mut lr = 1e-3f32;
    let mut hidden_dim = 64usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--file" => file = argv.next().expect("file"),
            "--per-class" => per_class = argv.next().expect("num").parse().expect("num"),
            "--epochs" => epochs = argv.next().expect("num").parse().expect("num"),
            "--binary" => binary = true,
            "--lr" => lr = argv.next().expect("num").parse().expect("num"),
            "--hidden" => hidden_dim = argv.next().expect("num").parse().expect("num"),
            other => panic!("未知参数 {other}"),
        }
    }

    // 1) 数据：26 类，每类下采样。
    let (all_labels, all_domains, family_names) = load_dataset(&file, binary);
    let mut rng = Rng::new(7);
    let keep = downsample(&all_labels, per_class, &mut rng);
    let labels: Vec<usize> = keep.iter().map(|&i| all_labels[i]).collect();
    let features: Vec<Vec<f32>> = keep.iter().map(|&i| featurize(&all_domains[i])).collect();
    let classes = if binary { 2 } else { family_names.len() + 1 };

    // 2) 分层划分（复用二分类版逻辑的思路，按 26 类分层）。
    let mut by_class: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, &label) in labels.iter().enumerate() {
        by_class.entry(label).or_default().push(index);
    }
    let mut train: Vec<usize> = Vec::new();
    let mut val: Vec<usize> = Vec::new();
    let mut test: Vec<usize> = Vec::new();
    for group in by_class.values_mut() {
        let n = group.len();
        for i in (1..n).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            group.swap(i, j);
        }
        let train_end = n * 7 / 10;
        let val_end = n * 85 / 100;
        train.extend(&group[..train_end]);
        val.extend(&group[train_end..val_end]);
        test.extend(&group[val_end..]);
    }
    println!(
        "{}",
        json!({
            "stage": "data",
            "classes": classes,
            "kept": keep.len(),
            "train": train.len(),
            "val": val.len(),
            "test": test.len(),
            "families": family_names.values().cloned().collect::<Vec<_>>(),
        })
    );

    // 3) 模型：2048→64(Tanh)→26（softmax 交叉熵），行 absmean 初始 scale。
    let ste = Ste::Clipped { ratio: 1.0 };
    let make_layer = |rows: usize, cols: usize, rng: &mut Rng| -> TernaryDense {
        let latent: Vec<Vec<f32>> = (0..rows)
            .map(|_| (0..cols).map(|_| rng.next_f32() * 0.05).collect())
            .collect();
        let absmean = |row: usize| {
            let mean = latent[row].iter().map(|w| w.abs()).sum::<f32>() / cols as f32;
            if mean > 1e-6 { mean } else { 1.0 }
        };
        TernaryDense::new(rows, cols, |r, c| latent[r][c], absmean, ste)
    };
    let mut hidden = make_layer(hidden_dim, features::FEAT_DIM, &mut rng);
    let mut out_layer = make_layer(classes, hidden_dim, &mut rng);
    let mut adam_h_latent = Adam::new(lr);
    let mut adam_h_scale = Adam::new(lr);
    let mut adam_o_latent = Adam::new(lr);
    let mut adam_o_scale = Adam::new(lr);

    // 4) 训练。
    for epoch in 0..epochs {
        let mut order = train.clone();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let mut total_loss = 0.0f32;
        for &index in &order {
            let h = hidden.forward(&features[index]);
            let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let logits = out_layer.forward(&h_act);
            let (loss, grad_logits) = cross_entropy(&logits, labels[index]);
            total_loss += loss;
            let grad_h_act = out_layer.backward(&grad_logits);
            out_layer.update(&mut adam_o_latent, &mut adam_o_scale);
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_h_latent, &mut adam_h_scale);
        }
        if epoch % 6 == 5 || epoch + 1 == epochs {
            let mut confusion = Confusion::new(classes);
            for &i in &val {
                let logits = forward_logits(&mut hidden, &mut out_layer, &features, i);
                let pred = logits
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
                    .map(|(c, _)| c)
                    .expect("non-empty");
                confusion.observe(pred, labels[i]);
            }
            println!(
                "{}",
                json!({
                    "stage": "train",
                    "epoch": epoch + 1,
                    "loss": total_loss / order.len() as f32,
                    "val_acc": confusion.accuracy(),
                    "val_macro_f1": confusion.macro_f1(),
                })
            );
        }
    }

    // 5) 测试集读数（QAT）。
    let argmax = |logits: &[f32]| -> usize {
        logits
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
            .map(|(c, _)| c)
            .expect("non-empty")
    };
    let mut clean_confusion = Confusion::new(classes);
    for &i in &test {
        let logits = forward_logits(&mut hidden, &mut out_layer, &features, i);
        clean_confusion.observe(argmax(&logits), labels[i]);
    }
    println!(
        "{}",
        json!({
            "stage": "test_qat",
            "acc": clean_confusion.accuracy(),
            "macro_f1": clean_confusion.macro_f1(),
        })
    );

    // 6) 导出 WSP 工件（量化权重 + scale，bias=0）并复核一致性。
    let build_layer = |dense: &TernaryDense| -> Layer {
        let trits: Vec<i8> = dense.quantized().into_iter().flatten().collect();
        Layer::with_scales(
            dense.rows(),
            dense.cols(),
            &trits,
            &vec![0i8; dense.rows()],
            dense.scales().to_vec(),
        )
        .expect("trained weights are valid")
    };
    // 工件推理：y = scale·(t·x)，隐层输出 Tanh。
    let artifact =
        Net::new(vec![build_layer(&hidden), build_layer(&out_layer)]).expect("chainable");
    let artifact_argmax = |net: &Net, x: &[f32]| -> usize {
        let mut v = x.to_vec();
        for layer in net.layers() {
            let cols = layer.cols();
            let mut next = vec![0.0f32; layer.rows()];
            for (row, out) in next.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for (c, t) in layer.trits()[row * cols..(row + 1) * cols]
                    .iter()
                    .enumerate()
                {
                    acc += f32::from(t.to_i8()) * v[c];
                }
                *out = layer.scales()[row] * acc;
            }
            v = next.iter().map(|&x| Activation::Tanh.forward(x)).collect();
        }
        argmax(&v)
    };
    let mut artifact_confusion = Confusion::new(classes);
    for &i in &test {
        artifact_confusion.observe(artifact_argmax(&artifact, &features[i]), labels[i]);
    }
    println!(
        "{}",
        json!({
            "stage": "test_artifact",
            "acc": artifact_confusion.accuracy(),
            "macro_f1": artifact_confusion.macro_f1(),
        })
    );

    // 7) 红队：比特翻转（随机）后的归因准确率。
    let total_trits: usize = artifact.layers().iter().map(|l| l.rows() * l.cols()).sum();
    let mut attack_report: Vec<Value> = Vec::new();
    for &k in &[1_000usize, 10_000, 50_000, 200_000] {
        let mut accs = Vec::new();
        for _ in 0..3 {
            let mut attacked = artifact.clone();
            for _ in 0..k {
                let li = (rng.next_u64() % attacked.layers().len() as u64) as usize;
                let layer = &attacked.layers()[li];
                let row = (rng.next_u64() % layer.rows() as u64) as usize;
                let col = (rng.next_u64() % layer.cols() as u64) as usize;
                let current = layer.trits()[row * layer.cols() + col].to_i8();
                let to = if current == 0 { 1 } else { 0 };
                Patch::new(vec![Edit::SetTrit {
                    layer: li,
                    row,
                    col,
                    to: Trit::from_i8(to).expect("valid"),
                }])
                .apply(&mut attacked)
                .expect("in-bounds");
            }
            let mut confusion = Confusion::new(classes);
            for &i in &test {
                confusion.observe(artifact_argmax(&attacked, &features[i]), labels[i]);
            }
            accs.push(confusion.accuracy());
        }
        accs.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        attack_report.push(json!({"k": k, "acc_median": accs[accs.len() / 2]}));
    }
    println!(
        "{}",
        json!({
            "stage": "redteam",
            "total_trits": total_trits,
            "clean_acc": clean_confusion.accuracy(),
            "attacks": attack_report,
            "worst_pairs": family_names
                .iter()
                .filter_map(|(&id, name)| {
                    let dga_pred_benign = clean_confusion.count(0, id);
                    (dga_pred_benign > 0).then(|| (name.clone(), dga_pred_benign))
                })
                .collect::<Vec<_>>(),
        })
    );
}
