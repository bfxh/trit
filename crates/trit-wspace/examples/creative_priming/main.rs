//! 三值创造性预激发：把 Albalak「创造性悖论」的解耦方案落到三进制红蓝对抗。
//!
//! **创造性悖论**（Albalak et al.）：直接用过去成功样本做奖励训练，模型复制
//! 过去——恰恰扼杀创造性。#497 的化石记忆已实证同构现象：精英锁死导致
//! 种群 best fitness 全程 0.7353 不再发现新结构；#495 的审计模型负结果
//! （acc 0.11，只认已知攻击）是同一悖论的防御侧表现。
//!
//! **解法：解耦过程与产品**。冻结攻击执行器（`Edit::SetTrit` 原语 + #492
//! 工件——不可训练），只训练**预激发模型 π_CP**（生成三值 primer = 攻击
//! 的过程性指导），奖励 = 新颖度 × 质量 × 簇间分散（**不含**对任何特定
//! 成功攻击的复制项）。
//!
//! 三段实验：
//! 1. **悖论复现**（对应论文 RL 组大小图）：直接强化攻击成功率，primer 组
//!    大小 G ∈ {8,16,32,64}——预期多样性塌缩（复制论文曲线形状）；
//! 2. **预激发解法**：同样的 G sweep，奖励换成 新颖度×质量 乘积——预期
//!    多样性不塌、成功率不降；
//! 3. **能量景观读数**：primer 群体的行覆盖熵（kill-switch = 单点深井，
//!    预激发后景观熵上升）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example creative_priming
//! ```

mod metrics;
mod target;

use metrics::{Rng, featurize, load_dataset, stratified_split};
use serde_json::{Value, json};
use target::{
    GENOME_LEN, Target, attack_quality, coverage_entropy, execute_primer, mean_hamming, median,
    train_target,
};

fn main() {
    let dataset = load_dataset("D:/KF/trit-data/sms-spam/SMSSpamCollection");
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, _val, test) = stratified_split(&labels, &mut rng);

    // 冻结执行器的目标（#492 口径训练后不再动）。
    let clean_target = train_target(&features, &labels, &train, 100);
    let clean_net = clean_target.net.clone();

    // —— 实验一：悖论复现（直接强化攻击成功率，G sweep）。 ——
    let mut paradox: Vec<Value> = Vec::new();
    for &g in &[8usize, 16, 32, 64] {
        // π_CP：简单起见用「reward 驱动的 primer 分布偏置」模拟 RL 组训练
        // （每代 G 个 primer 采样 → 按成功率加权更新偏置 → 下一代采样收敛）。
        let mut bias = vec![0.0f32; GENOME_LEN * 3];
        let mut f1s = Vec::new();
        let mut last_gen_primers: Vec<Vec<i8>> = Vec::new();
        for _gen in 0..4 {
            let mut primers = Vec::new();
            let mut rewards = Vec::new();
            for _ in 0..g {
                let mut primer = Vec::with_capacity(GENOME_LEN);
                for row in 0..GENOME_LEN {
                    let max_i = bias[row * 3..row * 3 + 3]
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
                        .map(|(i, _)| i)
                        .unwrap_or(1);
                    let jitter = if rng.next_f32() < 0.25 {
                        rng.next_index(3)
                    } else {
                        max_i
                    };
                    primer.push(match jitter {
                        0 => -1,
                        1 => 0,
                        _ => 1,
                    });
                }
                let mut t = Target {
                    net: clean_net.clone(),
                };
                let edits = execute_primer(&mut t, &primer);
                let quality = if edits > 0 {
                    attack_quality(&t, &clean_net, &features, &labels, &test)
                } else {
                    0.0
                };
                // 直接强化攻击成功率（复制过去成功——悖论触发条件）。
                for row in 0..GENOME_LEN {
                    let prio = primer[row];
                    let class = if prio == -1 {
                        0
                    } else if prio == 0 {
                        1
                    } else {
                        2
                    };
                    bias[row * 3 + class] += quality * 0.5;
                }
                primers.push(primer);
                rewards.push(quality);
                f1s.push(attack_quality(&t, &clean_net, &features, &labels, &test));
                let _ = &rewards;
            }
            last_gen_primers = primers;
        }
        let diversity = mean_hamming(&last_gen_primers);
        let cover = coverage_entropy(&last_gen_primers);
        let mut q = f1s.clone();
        paradox.push(json!({
            "group_size": g,
            "attack_f1_median": median(&mut q),
            "diversity_hamming": diversity,
            "coverage_entropy": cover,
        }));
    }

    // —— 实验二：预激发解法（奖励 = 新颖度 × 质量）。 ——
    let mut priming: Vec<Value> = Vec::new();
    for &g in &[8usize, 16, 32, 64] {
        let mut bias = vec![0.0f32; GENOME_LEN * 3];
        let mut f1s = Vec::new();
        let mut last_gen_primers: Vec<Vec<i8>> = Vec::new();
        for _gen in 0..4 {
            let mut primers: Vec<Vec<i8>> = Vec::new();
            for _ in 0..g {
                let mut primer = Vec::with_capacity(GENOME_LEN);
                for row in 0..GENOME_LEN {
                    let max_i = bias[row * 3..row * 3 + 3]
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
                        .map(|(i, _)| i)
                        .unwrap_or(1);
                    let jitter = if rng.next_f32() < 0.25 {
                        rng.next_index(3)
                    } else {
                        max_i
                    };
                    primer.push(match jitter {
                        0 => -1,
                        1 => 0,
                        _ => 1,
                    });
                }
                let mut t = Target {
                    net: clean_net.clone(),
                };
                let edits = execute_primer(&mut t, &primer);
                let quality = if edits > 0 {
                    attack_quality(&t, &clean_net, &features, &labels, &test)
                } else {
                    0.0
                };
                // **预激发奖励**：质量 × 新颖度（与历史 primers 的最小汉明距离）
                // —— 明确奖励「不是复制过去成功」。
                let novelty = if primers.is_empty() {
                    1.0
                } else {
                    let min_d = primers
                        .iter()
                        .map(|p| {
                            p.iter().zip(&primer).filter(|(a, b)| a != b).count() as f32
                                / GENOME_LEN as f32
                        })
                        .fold(f32::INFINITY, f32::min);
                    (min_d * 4.0).min(1.0)
                };
                let reward = quality * (0.5 + 0.5 * novelty);
                for row in 0..GENOME_LEN {
                    let prio = primer[row];
                    let class = if prio == -1 {
                        0
                    } else if prio == 0 {
                        1
                    } else {
                        2
                    };
                    bias[row * 3 + class] += reward * 0.5;
                }
                primers.push(primer);
                f1s.push(quality);
            }
            last_gen_primers = primers;
        }
        let diversity = mean_hamming(&last_gen_primers);
        let cover = coverage_entropy(&last_gen_primers);
        let mut q = f1s.clone();
        priming.push(json!({
            "group_size": g,
            "attack_f1_median": median(&mut q),
            "diversity_hamming": diversity,
            "coverage_entropy": cover,
        }));
    }

    println!(
        "{}",
        json!({
            "experiment": "paradox",
            "groups": paradox,
        })
    );
    println!(
        "{}",
        json!({
            "experiment": "priming",
            "groups": priming,
        })
    );
}
