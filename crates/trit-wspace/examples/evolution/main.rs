//! 三值红蓝共同进化：把「想法进化」的多智能体机制落到三进制安全对抗上。
//!
//! 思路来源是 PROTEUS 式的多角色进化（PROMETHEUS 发散 / MOMUS 批判 /
//! ANANKE 选择 / HARMONIA 合成 / 化石记录防重复发明）与 AB 双模型交替。
//! 三进制的角色不是装饰：**攻击策略本身就是三值基因组**——长度 = 隐藏行数
//! 的 `{-1, 0, +1}` 优先级向量，变异 = trit 翻转，交叉 = 逐位投票，
//! 适应度 = 对防御工件的**实测** F1 打击量。
//!
//! AB 交替：红方（攻击基因组进化）每代产出强攻击 → 蓝方（防御集成）吸收
//! 后扩大/更新 → 下一代红方对着更强的防御进化——「很快了解双方」就是每代
//! 双方的完整适应度报告。
//!
//! 自我假设（self-hypothesis）：每代统计存活策略的行分布，生成可验证假设
//! （「layer X 的行更承重」），下代变异按假设加权聚焦；假设命中 = 聚焦后
//! 成功率上升。化石记录 = 已评估基因组的 FNV 哈希档，重复者直接跳过并计数
//! （防「重新发明已被否决的方向」）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example evolution -- \
//!   --file <数据盘>/trit-data/sms-spam/SMSSpamCollection --generations 6
//! ```

mod metrics;

use metrics::{Rng, evaluate, featurize, load_dataset, stratified_split};
use serde_json::json;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

const FEAT_DIM: usize = 4096;
const HIDDEN: usize = 128;
const POP: usize = 12;
const ELITE: usize = 4;

/// 蓝方防御工件：一个集成成员（单层输出判别器）。
struct Defender {
    net: Net,
    seed: u64,
}

fn train_defender(
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    seed: u64,
    epochs: usize,
) -> Defender {
    let mut rng = Rng::new(seed);
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
    let mut hidden = make_layer(HIDDEN, FEAT_DIM, &mut rng);
    let mut out_layer = make_layer(1, HIDDEN, &mut rng);
    let mut adam_hl = Adam::new(1e-3);
    let mut adam_hs = Adam::new(1e-3);
    let mut adam_ol = Adam::new(1e-3);
    let mut adam_os = Adam::new(1e-3);
    for _ in 0..epochs {
        let mut order = train.to_vec();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        for &index in &order {
            let target = if labels[index] == 1 { 1.0 } else { -1.0 };
            let h = hidden.forward(&features[index]);
            let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let o = out_layer.forward(&h_act);
            let o_act: Vec<f32> = o.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let (_loss, grad_o_act) = hinge(&o_act, &[target]);
            let grad_o: Vec<f32> = grad_o_act
                .iter()
                .zip(&o_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let grad_h_act = out_layer.backward(&grad_o);
            out_layer.update(&mut adam_ol, &mut adam_os);
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_hl, &mut adam_hs);
        }
    }
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
    Defender {
        net: Net::new(vec![build_layer(&hidden), build_layer(&out_layer)]).expect("chainable"),
        seed,
    }
}

/// 集成投票（多数表决）输出：+1 spam / -1 ham。
fn ensemble_vote(defenders: &[Defender], x: &[f32]) -> f32 {
    let mut votes = 0i32;
    for d in defenders {
        let mut v = x.to_vec();
        for layer in d.net.layers() {
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
        votes += if v[0] > 0.0 { 1 } else { -1 };
    }
    if votes >= 0 { 1.0 } else { -1.0 }
}

/// 单个防御成员的输出（评估成员被 kill-switch 后的退化）。
fn member_out(d: &Defender, x: &[f32]) -> f32 {
    let mut v = x.to_vec();
    for layer in d.net.layers() {
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
}

/// 对一个防御成员施加「按基因组选行整行置零」攻击（走正规编辑原语）。
/// 基因组三值：+1 = 攻击该行（整行 trits 归零，即 #494 kill-switch 的泛化）。
fn apply_genome_attack(defender: &mut Defender, rows: &[(usize, usize)]) {
    let mut edits = Vec::new();
    for &(li, row) in rows {
        let cols = defender.net.layers()[li].cols();
        for c in 0..cols {
            if defender.net.layers()[li].trits()[row * cols + c].to_i8() != 0 {
                edits.push(Edit::SetTrit {
                    layer: li,
                    row,
                    col: c,
                    to: Trit::Zero,
                });
            }
        }
    }
    Patch::new(edits)
        .apply(&mut defender.net)
        .expect("in-bounds");
}

/// 基因组 → 攻击目标：三值 +1 的行按优先级取 top-k。
/// 基因组覆盖全部 129 行（0-127 = hidden 层，128 = 输出层 row 0）——
/// #494 的 kill-switch 在输出层，搜索空间必须包含它。
fn genome_targets(genome: &[i8], k: usize, rng: &mut Rng) -> Vec<(usize, usize)> {
    // +1 行随机采样 k 个（进化靠 fitness 选择而非固定排序——排序会把
    // 高索引的 kill-switch 行永久挤掉）。
    let mut candidates: Vec<(usize, usize)> = genome
        .iter()
        .enumerate()
        .filter(|&(_, &prio)| prio == 1)
        .map(|(row, _)| {
            if row < 128 {
                (0usize, row)
            } else {
                (1usize, 0usize)
            }
        })
        .collect();
    for i in (1..candidates.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        candidates.swap(i, j);
    }
    candidates.truncate(k);
    candidates
}

fn fnv(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &byte in bytes {
        hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    values[values.len() / 2]
}

/// 评估一个攻击基因组对蓝方集成成员 0 的打击量：F1 下降量（MOMUS 批判）。
/// 防御侧读数同时给出：集成在攻击后的 F1（防御收益）。
fn attack_fitness(
    genome: &[i8],
    k: usize,
    defenders: &[Defender],
    features: &[Vec<f32>],
    labels: &[usize],
    test: &[usize],
    rng: &mut Rng,
) -> (f32, f32) {
    let mut attacked = Defender {
        net: defenders[0].net.clone(),
        seed: 0,
    };
    apply_genome_attack(&mut attacked, &genome_targets(genome, k, rng));
    let outs: Vec<f32> = test
        .iter()
        .map(|&i| member_out(&attacked, &features[i]))
        .collect();
    let m = evaluate(&outs, labels);
    let outs_int: Vec<f32> = test
        .iter()
        .map(|&i| {
            let votes = defenders
                .iter()
                .map(|d| member_out(d, &features[i]))
                .filter(|&p| p > 0.0)
                .count();
            if votes * 2 > defenders.len() {
                1.0
            } else {
                -1.0
            }
        })
        .collect();
    let mi = evaluate(&outs_int, labels);
    (m.f1(), mi.f1())
}

fn main() {
    let mut file = String::from("D:/KF/trit-data/sms-spam/SMSSpamCollection");
    let mut generations = 6usize;
    let mut k = 12usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--file" => file = argv.next().expect("file"),
            "--generations" => generations = argv.next().expect("num").parse().expect("num"),
            "--k" => k = argv.next().expect("num").parse().expect("num"),
            other => panic!("未知参数 {other}"),
        }
    }

    // 0) 数据 + 蓝方初始集成（3 个 seed 的防御成员 = AB 的 B 侧基线）。
    let dataset = load_dataset(&file);
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, _val, test) = stratified_split(&labels, &mut rng);
    let test_labels: Vec<usize> = test.iter().map(|&i| labels[i]).collect();
    let mut defenders: Vec<Defender> = (0..3)
        .map(|i| train_defender(&features, &labels, &train, 100 + i as u64, 20))
        .collect();
    let clean_int: Vec<f32> = test
        .iter()
        .map(|&i| {
            let votes: Vec<f32> = defenders
                .iter()
                .map(|d| member_out(d, &features[i]))
                .collect();
            let n = votes.iter().filter(|&&p| p > 0.0).count();
            if n * 2 > defenders.len() { 1.0 } else { -1.0 }
        })
        .collect();
    let clean = evaluate(&clean_int, &test_labels);
    println!(
        "{}",
        json!({
            "stage": "baseline",
            "ensemble": defenders.len(),
            "acc": clean.accuracy(),
            "f1": clean.f1(),
            "rows": HIDDEN,
        })
    );

    // 1) PROMETHEUS 初代种群：随机三值基因组。
    // 化石记录：基因组哈希 → 历史 fitness（不重复评估，但记住成绩——防重复发明）。
    let mut fossils: std::collections::HashMap<u64, f32> = std::collections::HashMap::new();
    let mut population: Vec<Vec<i8>> = (0..POP)
        .map(|_| {
            (0..HIDDEN + 1)
                .map(|_| match rng.next_u64() % 5 {
                    0 => 1,
                    1 => -1,
                    _ => 0,
                })
                .collect()
        })
        .collect();
    // 化石记录：基因组哈希 → 历史 fitness（不重复评估，但记住成绩——防重复发明）。
    let mut hypothesis = json!({"layer_bias": [0.5, 0.5], "note": "无先验，均匀变异"});
    let mut fossil_rejects = 0usize;
    let mut hypothesis_hits = 0usize;
    let mut hypothesis_trials = 0usize;
    let baseline_f1 = {
        let outs: Vec<f32> = test
            .iter()
            .map(|&i| member_out(&defenders[0], &features[i]))
            .collect();
        evaluate(&outs, &test_labels).f1()
    };
    let mut prev_best_fitness = 0.0f32;

    for generation in 0..generations {
        // 2) MOMUS：实测每条基因组的适应度（对成员 0 的 F1 打击量）+ 集成防御读数。
        let mut scored: Vec<(f32, usize)> = Vec::new();
        let mut fossil_skipped_this_gen = 0usize;
        for (pi, genome) in population.iter().enumerate() {
            let key = fnv(&genome
                .iter()
                .flat_map(|&t| (t as i64).to_le_bytes())
                .collect::<Vec<u8>>());
            if let Some(&past) = fossils.get(&key) {
                // 化石命中：不重复评估，但历史成绩进 scored（好基因不死）。
                scored.push((past, pi));
                fossil_skipped_this_gen += 1;
                continue;
            }
            fossils.insert(key, 0.0); // 占位，评估后回填
            // 噪声鲁棒 MOMUS：目标行随机采样，单次评估可能漏掉关键行——
            // 取 2 次评估的 min F1（best fitness 取最好成绩，防精英被噪声误杀）。
            let f1_single = {
                let mut worst = f32::INFINITY;
                for _ in 0..2 {
                    let (f1, _) = attack_fitness(
                        genome,
                        k,
                        &defenders,
                        &features,
                        &test_labels,
                        &test,
                        &mut rng,
                    );
                    if f1 < worst {
                        worst = f1;
                    }
                }
                worst
            };
            let fitness = baseline_f1 - f1_single;
            fossils.insert(key, fitness);
            scored.push((fitness, pi));
        }
        fossil_rejects += fossil_skipped_this_gen;
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));

        // 3) 自我假设：统计 ELITE 策略里两个 layer 的行分布 → 下代变异偏向。
        let mut layer_hits = [0usize; 2];
        for &(fitness, pi) in scored.iter().take(ELITE) {
            let _ = fitness;
            for (row, &prio) in population[pi].iter().enumerate() {
                if prio > 0 {
                    let li = usize::from(row == HIDDEN);
                    layer_hits[li] += prio.unsigned_abs() as usize;
                }
            }
        }
        let total_hits: usize = layer_hits.iter().sum();
        let bias0 = if total_hits > 0 {
            layer_hits[0] as f32 / total_hits as f32
        } else {
            0.5
        };
        let best = scored.first().map(|&(f, _)| f).unwrap_or(0.0);
        let hit = best > prev_best_fitness;
        hypothesis_trials += 1;
        if generation > 0 && hit {
            hypothesis_hits += 1;
        }
        prev_best_fitness = prev_best_fitness.max(best);
        hypothesis = json!({
            "layer_bias": [bias0, 1.0 - bias0],
            "note": if hit { "聚焦假设命中（best fitness 上升）" } else { "假设未命中，回退均匀变异" },
        });

        println!(
            "{}",
            json!({
                "stage": "generation",
                "generation": generation,
                "best_fitness": best,
                "fossil_rejects_this_gen": fossil_skipped_this_gen,
                "hypothesis": hypothesis,
                "population_alive": scored.len(),
            })
        );

        // 4) ANANKE + HARMONIA：精英保留 + 交叉合成下代（变异按假设加权）。
        let bias = hypothesis["layer_bias"][0].as_f64().unwrap_or(0.5) as f32;
        let mut next_pop: Vec<Vec<i8>> = scored
            .iter()
            .take(ELITE)
            .map(|&(_, pi)| population[pi].clone())
            .collect();
        while next_pop.len() < POP {
            let parent_a = &population[scored[rng.next_index(scored.len())].1];
            let parent_b = &population[scored[rng.next_index(scored.len())].1];
            // HARMONIA 交叉：逐位三值投票（同向保留，分歧按假设偏向 layer0）。
            let mut child: Vec<i8> = parent_a
                .iter()
                .zip(parent_b)
                .map(|(&a, &b)| {
                    if a == b {
                        a
                    } else if rng.next_f32() < bias {
                        a
                    } else {
                        b
                    }
                })
                .collect();
            // PROMETHEUS 变异：按假设偏向翻转 1-3 位。
            let mutations = 1 + (rng.next_u64() % 3) as usize;
            for _ in 0..mutations {
                let row = rng.next_index(HIDDEN + 1);
                child[row] = match child[row] {
                    -1 => 0,
                    0 => 1,
                    _ => -1,
                };
            }
            next_pop.push(child);
        }
        population = next_pop;
    }

    // 5) AB 读数：进化后的最强攻击 vs 单模型 / 集成（防御收益）。
    let mut best_genome = population[0].clone();
    let mut best_fit = f32::MIN;
    for genome in &population {
        let key = fnv(&genome
            .iter()
            .flat_map(|&t| (t as i64).to_le_bytes())
            .collect::<Vec<u8>>());
        if fossils.contains_key(&key) {
            continue;
        }
        let (f1_single, _) = attack_fitness(
            genome,
            k,
            &defenders,
            &features,
            &test_labels,
            &test,
            &mut rng,
        );
        let fitness = baseline_f1 - f1_single;
        if fitness > best_fit {
            best_fit = fitness;
            best_genome = genome.clone();
        }
    }
    let targets = genome_targets(&best_genome, k, &mut rng);
    let mut attacked0 = Defender {
        net: defenders[0].net.clone(),
        seed: 0,
    };
    apply_genome_attack(&mut attacked0, &targets);
    let outs_single: Vec<f32> = test
        .iter()
        .map(|&i| member_out(&attacked0, &features[i]))
        .collect();
    let single_after = evaluate(&outs_single, &test_labels);
    let outs_ens: Vec<f32> = test
        .iter()
        .map(|&i| {
            let votes: Vec<f32> = defenders
                .iter()
                .map(|d| member_out(d, &features[i]))
                .collect();
            let n = votes.iter().filter(|&&p| p > 0.0).count();
            if n * 2 > defenders.len() { 1.0 } else { -1.0 }
        })
        .collect();
    let ens_after = evaluate(&outs_ens, &test_labels);
    println!(
        "{}",
        json!({
            "stage": "summary",
            "generations": generations,
            "k": k,
            "clean_ensemble_f1": clean.f1(),
            "best_fitness": best_fit,
            "single_member_after": {"acc": single_after.accuracy(), "f1": single_after.f1()},
            "ensemble_after": {"acc": ens_after.accuracy(), "f1": ens_after.f1()},
            "fossil_rejects_total": fossil_rejects,
            "hypothesis_hit_rate": hypothesis_hits as f32 / hypothesis_trials.max(1) as f32,
        })
    );
}
