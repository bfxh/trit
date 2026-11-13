//! 红队第二弹：对真实开源模型的三值工件的比特翻转攻击读数。
//!
//! 目标：EleutherAI/pythia-70m（F16 safetensors）的 6 层 MLP 链
//! （`dense_h_to_4h` 512→2048 → `dense_4h_to_h` 2048→512 ×6 = 12 层，
//! 共 2.1 亿 trit）。攻击通过**库自己的编辑原语**（`Edit::SetTrit` + `Patch`）
//! 落地——攻击者能力分级：
//!
//! - **随机攻击**：均匀随机翻转 k 个 trit（不知道权重，投毒/搅乱基线）；
//! - **按行方差攻击**：用输入统计挑贡献方差最大的行，行内翻一个 trit
//!   （便宜的白盒代理，代表「用了公开工具的聪明攻击者」）。
//!
//! 度量：输出 vs 干净三值输出的余弦与相对 L2（口径同 #447）。防篡改侧
//! （容器校验和/审计链）见 `docs/WSP-REDTEAM-STACK.md`。
//!
//! 用法：
//! ```bash
//! cargo run --release -p trit-wspace --example redteam_flip -- \
//!   --file pythia-70m.safetensors --inputs 128 --trials 10
//! ```

use serde_json::{Value, json};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::safetensors;
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

/// xorshift64* 确定性随机（同 seed 同攻击）。
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
    fn next_index(&mut self, bound: usize) -> usize {
        usize::try_from(self.next_u64() % bound as u64).expect("bound > 0")
    }
}

fn parse_args() -> (String, usize, usize) {
    let mut file = "pythia-70m.safetensors".to_string();
    let mut inputs = 128usize;
    let mut trials = 10usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--file" => file = argv.next().expect("file"),
            "--inputs" => inputs = argv.next().expect("num").parse().expect("num"),
            "--trials" => trials = argv.next().expect("num").parse().expect("num"),
            other => panic!("未知参数 {other}"),
        }
    }
    (file, inputs, trials)
}

/// 六层 MLP 链的 safetensors 名：512→2048→512 ×6。
fn layer_names() -> Vec<String> {
    let mut names = Vec::with_capacity(12);
    for layer in 0..6 {
        names.push(format!("gpt_neox.layers.{layer}.mlp.dense_h_to_4h.weight"));
        names.push(format!("gpt_neox.layers.{layer}.mlp.dense_4h_to_h.weight"));
    }
    names
}

/// 三值化一层：absmean（`Layer::from_f32`），bias 是 f32 幅度直读。
fn ternarize(file: &str, name: &str) -> Layer {
    let (weights, _vector) =
        safetensors::read_weights(file, name).unwrap_or_else(|error| panic!("读 {name}: {error}"));
    let bias_name = name
        .strip_suffix(".weight")
        .map(|stem| format!("{stem}.bias"));
    let bias = bias_name
        .and_then(|bias_name| safetensors::read_vector(file, &bias_name).ok())
        .unwrap_or_else(|| vec![0.0; weights.rows]);
    Layer::from_f32(weights.rows, weights.cols, &weights.values, &bias)
        .unwrap_or_else(|error| panic!("三值化 {name}: {error}"))
}

/// 前向：`y = scale·(t·x) + b`（f32 读数口径）。
fn forward(net: &Net, inputs: &[Vec<f32>]) -> Vec<Vec<f32>> {
    inputs
        .iter()
        .map(|x| {
            let mut v = x.clone();
            for layer in net.layers() {
                let cols = layer.cols();
                let mut next = vec![0.0f32; layer.rows()];
                for (row, out) in next.iter_mut().enumerate() {
                    let mut acc = 0.0f32;
                    for (col, t) in layer.trits()[row * cols..(row + 1) * cols]
                        .iter()
                        .enumerate()
                    {
                        acc += f32::from(t.to_i8()) * v[col];
                    }
                    *out = layer.scales()[row] * acc
                        + layer.bias_f32().get(row).copied().unwrap_or(0.0);
                }
                v = next;
            }
            v
        })
        .collect()
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0;
    let mut na = 0.0;
    let mut nb = 0.0;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    dot / (na.sqrt() * nb.sqrt()).max(1e-12)
}

fn rel_l2(a: &[f32], b: &[f32]) -> f32 {
    let mut diff = 0.0;
    let mut norm = 0.0;
    for (x, y) in a.iter().zip(b) {
        diff += (x - y) * (x - y);
        norm += y * y;
    }
    diff.sqrt() / norm.sqrt().max(1e-12)
}

/// 每行的贡献方差代理：`scale² · nnz`（输入假设 unit 方差）。
fn row_importance(layer: &Layer) -> Vec<f32> {
    let cols = layer.cols();
    layer
        .scales()
        .iter()
        .enumerate()
        .map(|(row, &scale)| {
            let nnz = layer.trits()[row * cols..(row + 1) * cols]
                .iter()
                .filter(|t| t.to_i8() != 0)
                .count() as f32;
            scale * scale * nnz
        })
        .collect()
}

/// 智能攻击的目标：全局 top-k 承重行，每行取首个非零 trit。
fn smart_targets(net: &Net, k: usize) -> Vec<(usize, usize, usize)> {
    let mut scored: Vec<(f32, usize, usize)> = Vec::new();
    for (li, layer) in net.layers().iter().enumerate() {
        for (row, &score) in row_importance(layer).iter().enumerate() {
            scored.push((score, li, row));
        }
    }
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));
    scored
        .iter()
        .take(k)
        .filter_map(|&(_, li, row)| {
            let cols = net.layers()[li].cols();
            net.layers()[li].trits()[row * cols..(row + 1) * cols]
                .iter()
                .position(|t| t.to_i8() != 0)
                .map(|col| (li, row, col))
        })
        .collect()
}

/// 翻转一个 trit 到不同的值（走库的正规编辑原语）。
fn flip(net: &mut Net, li: usize, row: usize, col: usize, rng: &mut Rng) {
    let current = net.layers()[li].trits()[row * net.layers()[li].cols() + col].to_i8();
    let mut to = rng.next_index(3) as i8 - 1;
    if to == current {
        to = (to + 2) % 3 - 1;
        if to == current {
            to = -current;
        }
    }
    let patch = Patch::new(vec![Edit::SetTrit {
        layer: li,
        row,
        col,
        to: Trit::from_i8(to).expect("valid trit"),
    }]);
    patch.apply(net).expect("in-bounds flip");
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    values[values.len() / 2]
}

fn main() {
    let (file, inputs_n, trials) = parse_args();
    let names = layer_names();
    let clean = Net::new(names.iter().map(|name| ternarize(&file, name)).collect())
        .expect("chainable layers");
    let total_trits: usize = clean.layers().iter().map(|l| l.rows() * l.cols()).sum();
    println!(
        "{}",
        json!({"stage": "loaded", "tensors": names.len(), "total_trits": total_trits})
    );

    let mut rng = Rng::new(7);
    let inputs: Vec<Vec<f32>> = (0..inputs_n)
        .map(|_| (0..512).map(|_| rng.next_f32()).collect())
        .collect();
    let baseline = forward(&clean, &inputs);

    let mut report: Vec<Value> = Vec::new();
    for &k in &[1usize, 10, 100, 1000, 10000] {
        let mut random_cos = Vec::with_capacity(trials);
        let mut random_l2 = Vec::with_capacity(trials);
        for _ in 0..trials {
            let mut attacked = clean.clone();
            for _ in 0..k {
                let li = rng.next_index(attacked.layers().len());
                let layer = &attacked.layers()[li];
                let (row, col) = (rng.next_index(layer.rows()), rng.next_index(layer.cols()));
                flip(&mut attacked, li, row, col, &mut rng);
            }
            let outs = forward(&attacked, &inputs);
            let n = inputs_n as f32;
            random_cos.push(
                baseline
                    .iter()
                    .zip(&outs)
                    .map(|(a, b)| cosine(a, b))
                    .sum::<f32>()
                    / n,
            );
            random_l2.push(
                baseline
                    .iter()
                    .zip(&outs)
                    .map(|(a, b)| rel_l2(a, b))
                    .sum::<f32>()
                    / n,
            );
        }
        // 按行方差攻击（k 行各 1 trit；目标集合对固定模型是确定性的）。
        let mut attacked = clean.clone();
        for &(li, row, col) in &smart_targets(&clean, k) {
            flip(&mut attacked, li, row, col, &mut rng);
        }
        let outs = forward(&attacked, &inputs);
        let n = inputs_n as f32;
        let smart_cos = baseline
            .iter()
            .zip(&outs)
            .map(|(a, b)| cosine(a, b))
            .sum::<f32>()
            / n;
        let smart_l2 = baseline
            .iter()
            .zip(&outs)
            .map(|(a, b)| rel_l2(a, b))
            .sum::<f32>()
            / n;
        report.push(json!({
            "k": k,
            "random_cos_median": median(&mut random_cos),
            "random_rel_l2_median": median(&mut random_l2),
            "rowvar_cos": smart_cos,
            "rowvar_rel_l2": smart_l2,
        }));
    }

    println!(
        "{}",
        json!({
            "inputs": inputs_n,
            "trials_per_k": trials,
            "total_trits": total_trits,
            "attacks": report,
        })
    );
}
