//! 神经防御部件：三值工件完整性审计模型。威胁模型（接 #489/#494）：攻击者
//! 可改写工件并**重算 FNV 校验和**——校验和对内容偷换无能为力，审计链
//! 无签名锚点同理。数据集全自动生成、标注免费（9 种攻击模式 + 合法样本，
//! 字节 4-gram 特征）。**负结果**：字节级特征对检测篡改无增量价值（校验和
//! 已 100%+零误报），正确战场是行为级指纹——读数与根因见
//! `docs/WSP-SEC-LOOP.md`。复算：`cargo run --release -p trit-wspace --example artifact_guard`。

mod metrics;

use metrics::{Rng, evaluate_binary};
use serde_json::json;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::cross_entropy;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::{Layer, Net};

const FEAT_DIM: usize = 2048;
const CLASSES: usize = 9;

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 构造一个「真实感」的多层工件（hidden 链 + 非平凡 scale/biasf）。
fn build_artifact(rng: &mut Rng) -> (Net, Vec<u8>) {
    let mut layers = Vec::new();
    let mut cols = 64;
    for rows in [128, 64, 32] {
        let mut trits = Vec::with_capacity(rows * cols);
        for _ in 0..rows * cols {
            trits.push(match rng.next_u64() % 5 {
                0 | 1 => -1i8,
                2 | 3 => 1,
                _ => 0,
            });
        }
        let scales: Vec<f32> = (0..rows)
            .map(|_| 0.1 + rng.next_f32().abs() * 0.9)
            .collect();
        layers
            .push(Layer::with_scales(rows, cols, &trits, &vec![0i8; rows], scales).expect("valid"));
        cols = rows;
    }
    let net = Net::new(layers).expect("chainable");
    let bytes = trit_wspace::wsp2::to_wsp2(&net).expect("encode");
    (net, bytes)
}

/// 9 类样本：0=合法，1-8=攻击模式。返回 (字节特征, 标签)。
/// **关键设计**：攻击样本从**固定工件池**取（每个样本对应池中某工件的变体），
/// 合法样本 = 池中工件原样——这样「合法 vs 攻击」的字节差异**恰好是攻击
/// delta 本身**，而不是工件间的随机差异（否则两类同分布，无判别信号——
/// 首轮实测误报率 100% 的教训）。全部模式都**重算校验和**。
fn make_sample(pool: &[Vec<u8>], class: usize, rng: &mut Rng) -> Vec<f32> {
    let mut bytes = pool[rng.next_u64() as usize % pool.len()].clone();
    let payload_end = bytes.len() - 8;
    match class {
        0 => {} // 合法
        1 => {
            // 单 trit blob 翻转（模拟 #489 的最小篡改）
            let pos = payload_end / 2 + rng.next_u64() as usize % 64;
            bytes[pos] ^= 0b0100_0000;
        }
        2 => {
            // 多点稀疏翻转
            for _ in 0..16 {
                let pos = payload_end / 4 + rng.next_u64() as usize % (payload_end / 2);
                bytes[pos] ^= 1 << (rng.next_u64() % 8);
            }
        }
        3 => {
            // scale 字段篡改：f32 位型改成 NaN 型（行为级破坏）
            let pos = 8 + rng.next_u64() as usize % 32;
            bytes[pos..pos + 4].copy_from_slice(&0x7fc0_0000u32.to_le_bytes());
        }
        4 => {
            // 结构字段篡改：layer count 翻位
            bytes[5] ^= 0x01;
        }
        5 => {
            // 大块清零（权重抹除攻击）
            let start = payload_end / 3;
            for b in &mut bytes[start..start + 256] {
                *b = 0;
            }
        }
        6 => {
            // 内容偷换：整段替换成另一网络的编码（最隐蔽）
            let other = pool[rng.next_u64() as usize % pool.len()].clone();
            let take = (payload_end - 16).min(other.len().saturating_sub(16));
            bytes[16..16 + take].copy_from_slice(&other[16..16 + take]);
        }
        7 => {
            // 尾部校验和之外的填充注入
            bytes.insert(payload_end / 2, 0xff);
        }
        _ => {
            // 位平移（对齐破坏）
            let start = payload_end / 2;
            for b in &mut bytes[start..payload_end] {
                *b = b.rotate_left(1);
            }
        }
    }
    // 重算校验和（class 7 的插入改变长度，统一按当前长度重算尾部）：
    // 让「校验和不匹配」这种平凡信号消失。
    let end = bytes.len();
    let checksum = fnv(&bytes[..end - 8]);
    bytes[end - 8..].copy_from_slice(&checksum.to_le_bytes());
    // 字节 4-gram 哈希特征（有符号存在性：桶命中 → ±1 由哈希高位决定，让特征
    // 不是纯稀疏 0/1，给三值量化留信息量）。
    let mut feats = vec![0.0f32; FEAT_DIM];
    if bytes.len() > 4 {
        for start in 0..=bytes.len() - 4 {
            let h = fnv(&bytes[start..start + 4]);
            let slot = (h % FEAT_DIM as u64) as usize;
            feats[slot] = if h >> 63 == 0 { 1.0 } else { -1.0 };
        }
    }
    feats
}

fn main() {
    let per_class = 260usize;
    let mut rng = Rng::new(7);
    // 固定工件池（8 个）：合法与攻击样本共享同一底层工件。
    let pool: Vec<Vec<u8>> = (0..8).map(|_| build_artifact(&mut rng).1).collect();
    // 生成数据集：类 8 是 holdout（训练不含，测试含）——检验未见攻击泛化。
    let holdout = 8usize;
    let mut train: Vec<(Vec<f32>, usize)> = Vec::new();
    let mut seen_test: Vec<(Vec<f32>, usize)> = Vec::new();
    let mut unseen_test: Vec<(Vec<f32>, usize)> = Vec::new();
    for class in 0..CLASSES {
        for i in 0..per_class {
            let sample = make_sample(&pool, class, &mut rng);
            let n = per_class * 7 / 10;
            if class == holdout {
                unseen_test.push((sample, class));
            } else if i < n {
                train.push((sample, class));
            } else {
                seen_test.push((sample, class));
            }
        }
    }
    println!(
        "{}",
        json!({
            "stage": "data",
            "classes": CLASSES,
            "holdout_class": holdout,
            "train": train.len(),
            "seen_test": seen_test.len(),
            "unseen_test": unseen_test.len(),
        })
    );

    // 训练：2048→64→9，softmax 交叉熵，Adam。
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
    let mut hidden = make_layer(64, FEAT_DIM, &mut rng);
    let mut out_layer = make_layer(CLASSES, 64, &mut rng);
    let mut adam_hl = Adam::new(1e-3);
    let mut adam_hs = Adam::new(1e-3);
    let mut adam_ol = Adam::new(1e-3);
    let mut adam_os = Adam::new(1e-3);
    for epoch in 0..24 {
        let mut order: Vec<usize> = (0..train.len()).collect();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let mut total_loss = 0.0f32;
        for &index in &order {
            let (x, label) = &train[index];
            let h = hidden.forward(x);
            let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            let logits = out_layer.forward(&h_act);
            let (loss, grad_logits) = cross_entropy(&logits, *label);
            total_loss += loss;
            let grad_h_act = out_layer.backward(&grad_logits);
            out_layer.update(&mut adam_ol, &mut adam_os);
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_hl, &mut adam_hs);
        }
        if epoch % 8 == 7 || epoch + 1 == 24 {
            println!(
                "{}",
                json!({"stage": "train", "epoch": epoch + 1, "loss": total_loss / order.len() as f32})
            );
        }
    }

    // 评测：已见攻击的检出 + 未见攻击的泛化（二值化：合法 vs 任意攻击）。
    let argmax = |logits: &[f32]| -> usize {
        logits
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
            .map(|(c, _)| c)
            .expect("non-empty")
    };
    let seen_preds: Vec<(usize, usize)> = seen_test
        .iter()
        .map(|(x, label)| {
            let h = hidden.forward(x);
            let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            (argmax(&out_layer.forward(&h_act)), *label)
        })
        .collect();
    let unseen_preds: Vec<(usize, usize)> = unseen_test
        .iter()
        .map(|(x, label)| {
            let h = hidden.forward(x);
            let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            (argmax(&out_layer.forward(&h_act)), *label)
        })
        .collect();
    // 已见：多类混淆。
    let mut seen_conf = trit_learn::metrics::Confusion::new(CLASSES);
    for (pred, label) in &seen_preds {
        seen_conf.observe(*pred, *label);
    }
    // 二值口径：合法(0) vs 攻击(1..9)——防御者最关心的读数。
    let seen_binary = evaluate_binary(&seen_preds);
    let unseen_binary = evaluate_binary(&unseen_preds);
    println!(
        "{}",
        json!({
            "stage": "eval",
            "seen": {
                "acc": seen_conf.accuracy(),
                "macro_f1": seen_conf.macro_f1(),
                "benign_vs_attack": seen_binary,
            },
            "unseen": {
                "benign_vs_attack": unseen_binary,
                "note": "类 8（位平移）训练时不可见",
            },
        })
    );
}
