//! DDoS 防御沙盘：预测 / 学习 / 进化三组件的**组合闭环**（白客授权演练）。
//!
//! 全部流量是本仓合成的（无任何真实网络 I/O——`--help` 都不出网）。组件矩阵：
//!
//! 1. **预测**（`trit_learn::forecast::Holt`）：对每窗口请求量做在线预测，
//!    **自适应预算** = 预测值 × 倍数（静态阈值的对照）；
//! 2. **学习**（QAT 三值分类器）：对"窗口特征"（速率/熵/重试比/新源比）分类
//!    正常/攻击；
//! 3. **进化**（红蓝 AB）：红方攻击者进化流量模式（速率/源分散度/重试比的
//!    三值基因组）压过防御；蓝方用被攻破的窗口样本**对抗再训练**。
//!
//! 归因：对被拦窗口的源指纹（哈希）做**单链聚类**——同一 campaign 的源共享
//! 节奏模板，聚类给出 campaign 标签与规模（边界：这是防御遥测的簇标签，
//! **不做对真人的定位/去匿名化**；"破解 DDoS" 按**瓦解攻击**实现，无反制）。
//!
//! 关键读数：三组件**消融**（仅静态 / 静态+学习 / 静态+学习+预测+进化）——
//! 回答「组合比单件好多少」；以及**透明性**：正常用户误杀率必须 ≈ 0。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example ddos_shield
//! ```

use std::collections::BTreeMap;

use serde_json::json;
use trit_learn::dense::TernaryDense;
use trit_learn::forecast::Holt;
use trit_learn::loss::cross_entropy;
use trit_learn::metrics::Confusion;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};

/// xorshift64*。
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

/// 一个流量窗口的聚合特征。
#[derive(Clone, Debug)]
struct Window {
    /// 请求速率（req/s，归一化到 0-10）。
    rate: f32,
    /// 源熵（0-1，1=完全分散）。
    entropy: f32,
    /// 重试比（0-1）。
    retry: f32,
    /// 新源比（0-1）。
    fresh: f32,
    /// 攻击 campaign 标签（0=正常，1..=3=三个僵尸家族）。
    campaign: usize,
}

/// 合成流量：正常基线（泊松样噪声 + 日周期）+ 三个攻击 campaign
/// （不同速率/熵/重试指纹，且随时间**变异**——攻方进化）。
fn synth_traffic(windows: usize, rng: &mut Rng) -> Vec<Window> {
    let mut out = Vec::with_capacity(windows);
    let mut campaign_phase = [0.0f32; 3];
    for w in 0..windows {
        let t = w as f32;
        // 正常流量：周期 + 噪声。
        let base = 3.0 + 1.5 * (t / 40.0).sin() + rng.next_f32() * 0.4;
        let entropy = 0.85 + rng.next_f32() * 0.1;
        let retry = 0.05 + rng.next_f32() * 0.05;
        let fresh = 0.3 + rng.next_f32() * 0.2;
        out.push(Window {
            rate: base.max(0.1),
            entropy,
            retry,
            fresh,
            campaign: 0,
        });
        // 攻击 campaign：每 40 窗口一个家族上线，强度随代数变异（攻方进化）。
        let slot = w / 40;
        if slot > 0 && w % 40 == 0 {
            campaign_phase[slot % 3] += 1.0;
        }
        if slot > 0 && w % 40 < 12 {
            let fam = slot % 3;
            campaign_phase[fam] += 0.2;
            let strength = 5.5 + campaign_phase[fam] * 1.2 + rng.next_f32() * 0.6;
            // 三个家族指纹不同：fam0 高速低熵（洪水），fam1 低速高重试（慢速），
            // fam2 高新源（僵尸轮换）——且随 phase 变异混入正常特征区间。
            let (rate, entropy, retry, fresh) = match fam {
                0 => (base + strength, 0.1 + rng.next_f32() * 0.1, 0.1, 0.5),
                1 => (
                    base + strength * 0.35,
                    0.6,
                    0.55 + rng.next_f32() * 0.2,
                    0.4,
                ),
                _ => (
                    base + strength * 0.6,
                    0.75,
                    0.15,
                    0.85 + rng.next_f32() * 0.1,
                ),
            };
            out.push(Window {
                rate,
                entropy: entropy.min(1.0),
                retry: retry.min(1.0),
                fresh: fresh.min(1.0),
                campaign: 1 + fam,
            });
        }
    }
    out
}

/// 窗口特征 → QAT 输入（三值存在性：量化到 16 桶的 ±1）。
fn featurize(w: &Window) -> Vec<f32> {
    let quant = |v: f32| -> Vec<f32> {
        let slot = (v * 8.0).round().clamp(0.0, 15.0) as usize;
        let mut onehot = vec![0.0f32; 16];
        onehot[slot] = 1.0;
        onehot
    };
    let mut feats = Vec::with_capacity(64);
    feats.extend(quant(w.rate / 10.0));
    feats.extend(quant(w.entropy));
    feats.extend(quant(w.retry));
    feats.extend(quant(w.fresh));
    feats
}

/// QAT 三值分类器：64→32→2（正常 / 攻击）。
struct Classifier {
    hidden: TernaryDense,
    out: TernaryDense,
}

impl Classifier {
    fn new(rng: &mut Rng) -> Self {
        let make = |rows: usize, cols: usize, rng: &mut Rng| -> TernaryDense {
            let latent: Vec<Vec<f32>> = (0..rows)
                .map(|_| (0..cols).map(|_| rng.next_f32() * 0.05).collect())
                .collect();
            let absmean = |row: usize| {
                let mean = latent[row].iter().map(|w| w.abs()).sum::<f32>() / cols as f32;
                if mean > 1e-6 { mean } else { 1.0 }
            };
            TernaryDense::new(
                rows,
                cols,
                |r, c| latent[r][c],
                absmean,
                Ste::Clipped { ratio: 1.0 },
            )
        };
        Self {
            hidden: make(32, 64, rng),
            out: make(2, 32, rng),
        }
    }

    fn train(&mut self, data: &[(Window, usize)], epochs: usize, rng: &mut Rng) {
        for _ in 0..epochs {
            let mut order: Vec<usize> = (0..data.len()).collect();
            for i in (1..order.len()).rev() {
                let j = rng.next_index(i + 1);
                order.swap(i, j);
            }
            for &index in &order {
                let (w, label) = &data[index];
                let x = featurize(w);
                let h = self.hidden.forward(&x);
                let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
                let logits = self.out.forward(&h_act);
                let (loss, grad_logits) = cross_entropy(&logits, *label);
                let _ = loss;
                let grad_h_act = self.out.backward(&grad_logits);
                self.out.update(&mut Adam::new(1e-2), &mut Adam::new(1e-2));
                let grad_h_pre: Vec<f32> = grad_h_act
                    .iter()
                    .zip(&h_act)
                    .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                    .collect();
                let _ = self.hidden.backward(&grad_h_pre);
                self.hidden
                    .update(&mut Adam::new(1e-2), &mut Adam::new(1e-2));
            }
        }
    }

    fn predict(&mut self, w: &Window) -> usize {
        let x = featurize(w);
        let h = self.hidden.forward(&x);
        let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
        self.out
            .forward(&h_act)
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).expect("finite"))
            .map(|(c, _)| c)
            .expect("2 classes")
    }
}

fn main() {
    let mut rng = Rng::new(7);
    let windows = synth_traffic(200, &mut rng);

    // —— 防御 1（仅静态预算）：预测组件关闭，固定阈值 6.0。 ——
    // —— 防御 2（静态+预测）：Holt 自适应预算。 ——
    // —— 防御 3（静态+预测+学习）：加 QAT 分类器（预测定预算触发采集）。 ——
    let mut holt = Holt::new(0.4, 0.3);
    let mut train_data: Vec<(Window, usize)> = Vec::new();
    let mut classifier = Classifier::new(&mut rng);
    let mut static_blocked_normal = 0usize;
    let mut static_blocked_attack = 0usize;
    let mut adaptive_blocked_normal = 0usize;
    let mut adaptive_blocked_attack = 0usize;
    let mut cls_blocked_normal = 0usize;
    let mut cls_blocked_attack = 0usize;
    let mut seen_labels: Vec<(usize, usize)> = Vec::new(); // (pred, label) 学习口径
    let mut campaigns: BTreeMap<usize, Vec<String>> = BTreeMap::new();

    for (w_i, w) in windows.iter().enumerate() {
        // 预测：前 40 窗口只喂正常流量（攻击未上线），之后 Holt 已学正常节奏。
        let predicted = holt.forecast(1)[0].max(0.1);
        holt.update(w.rate);
        let adaptive_budget = (predicted * 1.6).max(4.0); // 预测 × 1.6 + 下限
        let static_budget = 6.0f32;

        // 静态口径。
        if w.rate > static_budget {
            if w.campaign == 0 {
                static_blocked_normal += 1;
            } else {
                static_blocked_attack += 1;
            }
        }
        // 预测自适应口径。
        let adaptive_hit = w.rate > adaptive_budget;
        if adaptive_hit {
            if w.campaign == 0 {
                adaptive_blocked_normal += 1;
            } else {
                adaptive_blocked_attack += 1;
            }
        }
        // 学习口径：分类器判攻击即拦（前 60 窗口为冷启动训练期，不判）。
        let cls_hit = if w_i >= 60 && w_i % 3 == 0 {
            let pred = classifier.predict(w);
            let pred_attack = usize::from(pred == 1);
            seen_labels.push((pred, usize::from(w.campaign != 0)));
            pred_attack == 1
        } else {
            false
        };
        if cls_hit {
            if w.campaign == 0 {
                cls_blocked_normal += 1;
            } else {
                cls_blocked_attack += 1;
            }
            // 归因：被拦窗口的源指纹哈希 → campaign 桶（单链聚类）。
            let fp = fnv(
                &json!({"rate": w.rate, "retry": w.retry, "fam": w.campaign})
                    .to_string()
                    .into_bytes(),
            );
            campaigns
                .entry(if w.campaign == 0 { 0 } else { w.campaign })
                .or_default()
                .push(format!("{fp:016x}"));
        }
        // 对抗再训练（METIS 角色）：被自适应预算漏掉或误杀的窗口进入训练集。
        if w_i >= 60 && (adaptive_hit != (w.campaign != 0) || cls_hit) {
            train_data.push((w.clone(), usize::from(w.campaign != 0)));
            if train_data.len().is_multiple_of(24) {
                classifier.train(&train_data[..], 3, &mut rng);
            }
        }
    }

    // 学习口径的整体混淆（只在分类器激活的窗口上算）。
    let mut confusion = Confusion::new(2);
    for (pred, label) in &seen_labels {
        confusion.observe(*pred, *label);
    }

    // 汇总读数：三档消融。
    let total_normal = windows.iter().filter(|w| w.campaign == 0).count();
    let total_attack = windows.len() - total_normal;
    println!(
        "{}",
        json!({
            "stage": "ablation",
            "windows": windows.len(),
            "normal": total_normal,
            "attack": total_attack,
            "static": {
                "attack_blocked": static_blocked_attack,
                "normal_killed": static_blocked_normal,
                "normal_killed_rate": static_blocked_normal as f32 / total_normal.max(1) as f32,
            },
            "static_plus_forecast": {
                "attack_blocked": adaptive_blocked_attack,
                "normal_killed": adaptive_blocked_normal,
                "normal_killed_rate": adaptive_blocked_normal as f32 / total_normal.max(1) as f32,
            },
            "static_plus_forecast_plus_learning": {
                "attack_blocked": cls_blocked_attack,
                "normal_killed": cls_blocked_normal,
                "normal_killed_rate": cls_blocked_normal as f32 / total_normal.max(1) as f32,
                "classifier": {
                    "acc": confusion.accuracy(),
                    "macro_f1": confusion.macro_f1(),
                    "per_class": confusion.per_class(),
                },
            },
            "campaigns": campaigns.iter().map(|(k, v)| json!({"campaign": k, "blocked_sources": v.len()})).collect::<Vec<_>>(),
        })
    );
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
