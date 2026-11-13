//! 三值 QAT 训练 + 工件红队：UCI SMS Spam（真实数据集，白客防御任务）全链。
//!
//! 管线（全部 Rust，无 Python——训练禁 Python 硬门一致）：
//! 1. **数据**：UCI SMS Spam Collection（5,574 条，`ham/spam<TAB>text`）；
//! 2. **特征**（自造部件）：字符 3-gram/4-gram FNV 哈希到 4096 维，出现=+1
//!    ⇒ **输入本身三值**（存在性特征），无预训练 embedding；
//! 3. **模型**：QAT 三值 MLP `4096→128→1`（`TernaryDense` + `Ste::Clipped`，
//!    Tanh 隐层 / Tanh 输出，hinge 损失，Adam(latent)+Adam(scale)）；
//! 4. **评测**（自造部件）：分层 70/15/15，acc / precision / recall / F1
//!    （spam=正类）+ 混淆矩阵；
//! 5. **导出**：量化权重 → `trit-wspace` `Layer`（scale 语义不变，bias=0）
//!    → WSP2 工件；
//! 6. **红队**（接 #489 的比特翻转线）：在**有真实任务指标**的工件上做
//!    比特翻转攻击——翻转 k 个 trit 后 spam 检出率/F1 掉多少。
//!
//! 用法：
//! ```bash
//! cargo run --release -p trit-wspace --example spam_redteam -- \
//!   --file D:/KF/trit-data/sms-spam/SMSSpamCollection
//! ```

use std::fs;

use serde_json::{Value, json};
use trit_learn::Activation;
use trit_learn::Ste;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::opt::Optimizer;
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

const FEAT_DIM: usize = 4096;
const HIDDEN: usize = 128;

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

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 加载 `label<TAB>text`；ham=0、spam=1。
fn load_dataset(path: &str) -> Vec<(usize, String)> {
    let text = fs::read_to_string(path).unwrap_or_else(|error| panic!("读 {path}: {error}"));
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (label, body) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("行不是 label<TAB>text：{line:.40}"));
            let label = match label.trim() {
                "ham" => 0,
                "spam" => 1,
                other => panic!("未知标签 {other}"),
            };
            (label, body.to_string())
        })
        .collect()
}

/// 字符 3-gram/4-gram 哈希特征：出现=+1（三值输入的存在性编码）。
fn featurize(text: &str) -> Vec<f32> {
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let mut feats = vec![0.0f32; FEAT_DIM];
    for width in [3usize, 4] {
        if lower.len() < width {
            continue;
        }
        for start in 0..=(lower.len() - width) {
            let mut bytes = Vec::with_capacity(width * 2);
            for ch in &lower[start..start + width] {
                let mut buf = [0u8; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            let slot = (fnv(&bytes) % FEAT_DIM as u64) as usize;
            feats[slot] = 1.0;
        }
    }
    feats
}

/// 分层划分：每个类别内 shuffle 后按 70/15/15 切。
fn stratified_split(labels: &[usize], rng: &mut Rng) -> (Vec<usize>, Vec<usize>, Vec<usize>) {
    let mut by_class: [Vec<usize>; 2] = [Vec::new(), Vec::new()];
    for (index, &label) in labels.iter().enumerate() {
        by_class[label].push(index);
    }
    let mut train = Vec::new();
    let mut val = Vec::new();
    let mut test = Vec::new();
    for group in &mut by_class {
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
    (train, val, test)
}

struct Metrics {
    tp: usize,
    fp: usize,
    fn_: usize,
    tn: usize,
}

fn evaluate(net_outs: &[f32], labels: &[usize]) -> Metrics {
    let mut m = Metrics {
        tp: 0,
        fp: 0,
        fn_: 0,
        tn: 0,
    };
    for (out, &label) in net_outs.iter().zip(labels) {
        let pred = usize::from(*out > 0.0);
        match (pred, label) {
            (1, 1) => m.tp += 1,
            (1, 0) => m.fp += 1,
            (0, 1) => m.fn_ += 1,
            _ => m.tn += 1,
        }
    }
    m
}

impl Metrics {
    fn accuracy(&self) -> f32 {
        let total = self.tp + self.fp + self.fn_ + self.tn;
        (self.tp + self.tn) as f32 / total as f32
    }
    fn precision(&self) -> f32 {
        self.tp as f32 / (self.tp + self.fp).max(1) as f32
    }
    fn recall(&self) -> f32 {
        self.tp as f32 / (self.tp + self.fn_).max(1) as f32
    }
    fn f1(&self) -> f32 {
        let p = self.precision();
        let r = self.recall();
        2.0 * p * r / (p + r).max(1e-12)
    }
    fn to_value(&self) -> Value {
        json!({
            "accuracy": self.accuracy(),
            "precision": self.precision(),
            "recall": self.recall(),
            "f1": self.f1(),
            "confusion": {"tp": self.tp, "fp": self.fp, "fn": self.fn_, "tn": self.tn},
        })
    }
}

fn main() {
    let mut file = String::from("D:/KF/trit-data/sms-spam/SMSSpamCollection");
    let mut epochs = 30usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--file" => file = argv.next().expect("file"),
            "--epochs" => epochs = argv.next().expect("num").parse().expect("num"),
            other => panic!("未知参数 {other}"),
        }
    }

    // 1) 数据。
    let dataset = load_dataset(&file);
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, val, test) = stratified_split(&labels, &mut rng);
    println!(
        "{}",
        json!({
            "stage": "data",
            "total": dataset.len(),
            "spam": labels.iter().filter(|&&l| l == 1).count(),
            "train": train.len(),
            "val": val.len(),
            "test": test.len(),
        })
    );

    // 2) 模型：4096→128(Tanh)→1(Tanh)，QAT + STE。
    let ste = Ste::Clipped { ratio: 1.0 };
    // 初始化：小尺度均匀 latent；scale0 取**行 absmean**（与 `Layer::from_f32`
    // 同口径）——否则 latent=±0.05、scale=1 时整层量化成 0，网络是死的。
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
    let mut hidden = make_layer(HIDDEN, FEAT_DIM, &mut rng);
    let mut out_layer = make_layer(1, HIDDEN, &mut rng);
    let mut adam_h_latent = Adam::new(1e-3);
    let mut adam_h_scale = Adam::new(1e-3);
    let mut adam_o_latent = Adam::new(1e-3);
    let mut adam_o_scale = Adam::new(1e-3);

    // 3) 训练（单样本 SGD，逐 epoch 洗牌）。
    for epoch in 0..epochs {
        let mut order = train.clone();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let mut total_loss = 0.0f32;
        for &index in &order {
            let target = if labels[index] == 1 { 1.0 } else { -1.0 };
            let h = hidden.forward(&features[index]);
            let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let o = out_layer.forward(&h_act);
            let o_act: Vec<f32> = o.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let (loss, grad_o_act) = hinge(&o_act, &[target]);
            total_loss += loss;
            // 输出层反向。
            let grad_o: Vec<f32> = grad_o_act
                .iter()
                .zip(&o_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let grad_h_act = out_layer.backward(&grad_o);
            out_layer.update(&mut adam_o_latent, &mut adam_o_scale);
            // 隐层反向。
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_h_latent, &mut adam_h_scale);
        }
        if epoch % 5 == 4 || epoch + 1 == epochs {
            let val_outs: Vec<f32> = val
                .iter()
                .map(|&i| {
                    let h = hidden.forward(&features[i]);
                    let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
                    Activation::Tanh.forward(out_layer.forward(&h_act)[0])
                })
                .collect();
            let metrics = evaluate(
                &val_outs,
                &val.iter().map(|&i| labels[i]).collect::<Vec<_>>(),
            );
            println!(
                "{}",
                json!({"stage": "train", "epoch": epoch + 1, "loss": total_loss / order.len() as f32, "val": metrics.to_value()})
            );
        }
    }

    // 4) 测试集读数（QAT 模型）。
    let test_outs: Vec<f32> = test
        .iter()
        .map(|&i| {
            let h = hidden.forward(&features[i]);
            let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            Activation::Tanh.forward(out_layer.forward(&h_act)[0])
        })
        .collect();
    let test_labels: Vec<usize> = test.iter().map(|&i| labels[i]).collect();
    let clean_metrics = evaluate(&test_outs, &test_labels);
    println!(
        "{}",
        json!({"stage": "test_qat", "metrics": clean_metrics.to_value()})
    );

    // 5) 导出为 trit-wspace 工件（量化权重 + scale，bias=0）。
    let build_layer = |dense: &TernaryDense| -> Layer {
        let rows = dense.rows();
        let cols = dense.cols();
        let mut trits = Vec::with_capacity(rows * cols);
        for row in dense.quantized() {
            trits.extend(row);
        }
        Layer::with_scales(
            rows,
            cols,
            &trits,
            &vec![0i8; rows],
            dense.scales().to_vec(),
        )
        .expect("trained weights are valid")
    };
    // 量化推理口径（工件推理）：y = scale·(t·x)，输出取 tanh。
    let artifact_infer = |net: &Net, x: &[f32]| -> f32 {
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
        v[0]
    };
    let artifact =
        Net::new(vec![build_layer(&hidden), build_layer(&out_layer)]).expect("chainable");
    let artifact_outs: Vec<f32> = test
        .iter()
        .map(|&i| artifact_infer(&artifact, &features[i]))
        .collect();
    let artifact_metrics = evaluate(&artifact_outs, &test_labels);
    println!(
        "{}",
        json!({"stage": "test_artifact", "metrics": artifact_metrics.to_value()})
    );

    // 6) 红队：翻转 k 个 trit（随机，5 次重复）后 spam F1/漏检率。
    let total_trits: usize = artifact.layers().iter().map(|l| l.rows() * l.cols()).sum();
    let mut attack_report: Vec<Value> = Vec::new();
    for &k in &[1_000usize, 10_000, 100_000, 500_000] {
        let mut f1s = Vec::new();
        let mut misses = Vec::new();
        for _ in 0..5 {
            let mut attacked = artifact.clone();
            for _ in 0..k {
                let li = (rng.next_u64() % attacked.layers().len() as u64) as usize;
                let layer = &attacked.layers()[li];
                let row = (rng.next_u64() % layer.rows() as u64) as usize;
                let col = (rng.next_u64() % layer.cols() as u64) as usize;
                let current = layer.trits()[row * layer.cols() + col].to_i8();
                let to = match current {
                    -1 => 0,
                    0 => 1,
                    _ => 0,
                };
                Patch::new(vec![Edit::SetTrit {
                    layer: li,
                    row,
                    col,
                    to: Trit::from_i8(to).expect("valid"),
                }])
                .apply(&mut attacked)
                .expect("in-bounds");
            }
            let outs: Vec<f32> = test
                .iter()
                .map(|&i| artifact_infer(&attacked, &features[i]))
                .collect();
            let metrics = evaluate(&outs, &test_labels);
            f1s.push(metrics.f1());
            // 漏检率：spam 被判成 ham 的比例。
            misses.push(metrics.fn_ as f32 / (metrics.fn_ + metrics.tp).max(1) as f32);
        }
        f1s.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        misses.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        attack_report.push(json!({
            "k": k,
            "f1_median": f1s[f1s.len() / 2],
            "spam_miss_rate_median": misses[misses.len() / 2],
        }));
    }
    println!(
        "{}",
        json!({
            "stage": "redteam",
            "total_trits": total_trits,
            "clean_f1": clean_metrics.f1(),
            "attacks": attack_report,
        })
    );
}
