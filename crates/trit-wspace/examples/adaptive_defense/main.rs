//! 自适应防御：防御侧 π_CP 的完整实现——**探针生成器** + **对抗集成**。
//!
//! 两部分（接 #494/#495/#497/#499）：
//!
//! **Part A 自适应探针**：#495 方向修正的最后一步。探针生成器（π_CP 角色）
//! 以「让嫌疑实例与基线的分歧最大化」为目标训练——吃 #492-#495 的攻击样本
//! （8 种篡改模式自动生成、标注免费）。读数：达到与固定探针集相同检出率
//! 所需的探针数量（效率曲线），以及对 holdout 攻击模式的泛化。
//!
//! **Part B 对抗集成**：#499 对抗训练（RowDropout 注入）的成员进 #497 集成
//! 投票。读数：进化最强攻击（#497 基因组）下，普通集成 vs 对抗集成的 F1
//! 保持——两者正交性（#499 未证栏）的定量验证。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example adaptive_defense
//! ```

mod adv_train;
mod ensemble;
mod metrics;
mod target;

use adv_train::{apply_rows, train_epochs};
use ensemble::{ensemble_evaluate, probe_response};
use metrics::{Rng, featurize, fnv, load_dataset, stratified_split};
use serde_json::{Value, json};
use target::{evaluate, train_fresh};
use trit_wspace::Net;
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;

const PROBES: usize = 50;

/// 攻击模式枚举（#495 artifact_guard 的同款家族，用于探针生成器的训练数据）。
const ATTACK_PATTERNS: usize = 8;

/// 对工件施加一种攻击模式（重算校验和的偷换家族）。
fn apply_attack_pattern(net: &Net, pattern: usize, rng: &mut Rng) -> Net {
    let mut attacked = net.clone();
    let h_cols = attacked.layers()[0].cols();
    match pattern % ATTACK_PATTERNS {
        0 => {
            // 单 trit 翻转
            let row = rng.next_index(attacked.layers()[0].rows());
            let col = rng.next_index(h_cols);
            let cur = attacked.layers()[0].trits()[row * h_cols + col].to_i8();
            let to = if cur == 0 { 1 } else { 0 };
            Patch::new(vec![Edit::SetTrit {
                layer: 0,
                row,
                col,
                to: Trit::from_i8(to).expect("v"),
            }])
            .apply(&mut attacked)
            .expect("in-bounds");
        }
        1 => {
            // 多点稀疏翻转
            for _ in 0..16 {
                let row = rng.next_index(attacked.layers()[0].rows());
                let col = rng.next_index(h_cols);
                let cur = attacked.layers()[0].trits()[row * h_cols + col].to_i8();
                let to = if cur == 0 { 1 } else { 0 };
                Patch::new(vec![Edit::SetTrit {
                    layer: 0,
                    row,
                    col,
                    to: Trit::from_i8(to).expect("v"),
                }])
                .apply(&mut attacked)
                .expect("in-bounds");
            }
        }
        2 => {
            // 整行置零（kill-switch）
            let row = rng.next_index(attacked.layers()[0].rows());
            let mut edits = Vec::new();
            for c in 0..h_cols {
                if attacked.layers()[0].trits()[row * h_cols + c].to_i8() != 0 {
                    edits.push(Edit::SetTrit {
                        layer: 0,
                        row,
                        col: c,
                        to: Trit::Zero,
                    });
                }
            }
            Patch::new(edits).apply(&mut attacked).expect("in-bounds");
        }
        3 => {
            // 输出层部分反转（真后门，30%）
            let o_cols = attacked.layers()[1].cols();
            let mut flipped = 0usize;
            for c in 0..o_cols {
                if flipped * 10 >= o_cols * 3 {
                    break;
                }
                let cur = attacked.layers()[1].trits()[c].to_i8();
                if cur != 0 {
                    let to = match cur {
                        -1 => Trit::Pos,
                        1 => Trit::Neg,
                        _ => Trit::Zero,
                    };
                    Patch::new(vec![Edit::SetTrit {
                        layer: 1,
                        row: 0,
                        col: c,
                        to,
                    }])
                    .apply(&mut attacked)
                    .expect("in-bounds");
                    flipped += 1;
                }
            }
        }
        4 => {
            // 输出层整行置零（完全熄火）
            let mut edits = Vec::new();
            for c in 0..attacked.layers()[1].cols() {
                if attacked.layers()[1].trits()[c].to_i8() != 0 {
                    edits.push(Edit::SetTrit {
                        layer: 1,
                        row: 0,
                        col: c,
                        to: Trit::Zero,
                    });
                }
            }
            Patch::new(edits).apply(&mut attacked).expect("in-bounds");
        }
        5 => {
            // 行交换 ×2（结构重组）
            let rows = attacked.layers()[0].rows();
            for _ in 0..2 {
                let a = rng.next_index(rows);
                let b = rng.next_index(rows);
                let mut edits = Vec::new();
                for c in 0..h_cols {
                    let ta = attacked.layers()[0].trits()[a * h_cols + c].to_i8();
                    let tb = attacked.layers()[0].trits()[b * h_cols + c].to_i8();
                    if ta != tb {
                        edits.push(Edit::SetTrit {
                            layer: 0,
                            row: a,
                            col: c,
                            to: Trit::from_i8(tb).expect("v"),
                        });
                        edits.push(Edit::SetTrit {
                            layer: 0,
                            row: b,
                            col: c,
                            to: Trit::from_i8(ta).expect("v"),
                        });
                    }
                }
                let sa = attacked.layers()[0].scales()[a];
                let sb = attacked.layers()[0].scales()[b];
                edits.push(Edit::SetScale {
                    layer: 0,
                    row: a,
                    to: sb,
                });
                edits.push(Edit::SetScale {
                    layer: 0,
                    row: b,
                    to: sa,
                });
                Patch::new(edits).apply(&mut attacked).expect("in-bounds");
            }
        }
        6 => {
            // 行取反（NegateRow 语义的编辑版）
            let row = rng.next_index(attacked.layers()[0].rows());
            let mut edits = Vec::new();
            for c in 0..h_cols {
                let cur = attacked.layers()[0].trits()[row * h_cols + c].to_i8();
                if cur != 0 {
                    edits.push(Edit::SetTrit {
                        layer: 0,
                        row,
                        col: c,
                        to: Trit::from_i8(-cur).expect("v"),
                    });
                }
            }
            Patch::new(edits).apply(&mut attacked).expect("in-bounds");
        }
        _ => {
            // scale 扰动（读数漂移）
            let row = rng.next_index(attacked.layers()[0].rows());
            let new_scale = attacked.layers()[0].scales()[row] * 3.0;
            Patch::new(vec![Edit::SetScale {
                layer: 0,
                row,
                to: new_scale,
            }])
            .apply(&mut attacked)
            .expect("in-bounds");
        }
    }
    attacked
}

fn evolve_attack(
    net: &Net,
    k: usize,
    features: &[Vec<f32>],
    labels: &[usize],
    test: &[usize],
    rng: &mut Rng,
) -> Vec<(usize, usize)> {
    let (_base_acc, base_f1) = evaluate(net, features, labels, test);
    let mut population: Vec<Vec<i8>> = (0..12)
        .map(|_| {
            (0..net.layers()[0].rows() + 1)
                .map(|_| match rng.next_u64() % 5 {
                    0 => 1,
                    1 => -1,
                    _ => 0,
                })
                .collect()
        })
        .collect();
    let mut fossils: std::collections::HashMap<u64, f32> = std::collections::HashMap::new();
    let mut best_rows: Vec<(usize, usize)> = Vec::new();
    let mut best_fit = f32::MIN;
    for _ in 0..4 {
        let mut scored: Vec<(f32, usize)> = Vec::new();
        for (pi, genome) in population.iter().enumerate() {
            let key = fnv(&genome
                .iter()
                .flat_map(|&t| (t as i64).to_le_bytes())
                .collect::<Vec<u8>>());
            if let Some(&past) = fossils.get(&key) {
                scored.push((past, pi));
                continue;
            }
            let mut candidates: Vec<(usize, usize)> = genome
                .iter()
                .enumerate()
                .filter(|&(_, &prio)| prio == 1)
                .map(|(row, _)| {
                    if row < net.layers()[0].rows() {
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
            let mut attacked = net.clone();
            let mut edits = Vec::new();
            for &(li, row) in &candidates {
                let cols = attacked.layers()[li].cols();
                for c in 0..cols {
                    if attacked.layers()[li].trits()[row * cols + c].to_i8() != 0 {
                        edits.push(Edit::SetTrit {
                            layer: li,
                            row,
                            col: c,
                            to: Trit::Zero,
                        });
                    }
                }
            }
            Patch::new(edits).apply(&mut attacked).expect("in-bounds");
            let (_acc, f1) = evaluate(&attacked, features, labels, test);
            let fitness = base_f1 - f1;
            fossils.insert(key, fitness);
            scored.push((fitness, pi));
            if fitness > best_fit {
                best_fit = fitness;
                best_rows = candidates;
            }
        }
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));
        let mut next_pop: Vec<Vec<i8>> = scored
            .iter()
            .take(4)
            .map(|&(_, pi)| population[pi].clone())
            .collect();
        while next_pop.len() < 12 {
            let a = &population[scored[rng.next_index(scored.len())].1];
            let b = &population[scored[rng.next_index(scored.len())].1];
            let child: Vec<i8> = a
                .iter()
                .zip(b)
                .map(|(&x, &y)| if x == y || rng.next_f32() < 0.5 { x } else { y })
                .collect();
            next_pop.push(child);
        }
        population = next_pop;
    }
    if best_rows.is_empty() {
        for _ in 0..k {
            let row = rng.next_index(net.layers()[0].rows());
            best_rows.push((0, row));
        }
    }
    best_rows
}

fn main() {
    let dataset = load_dataset("D:/KF/trit-data/sms-spam/SMSSpamCollection");
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, val, test) = stratified_split(&labels, &mut rng);

    // 集成成员池（蓝方起点；plain[0] 同时是 Part A 的防御对象工件）。
    // **可行性门**：不平衡 hinge 有退化模式（种子不好时塌缩到全 ham——
    // 首轮运行实测 seed 102/103 的 F1 恰为 0，多数投票被拖死）。成员进集成
    // 前必须在 val 上过 F1 ≥ 0.5 的筛查——这是集成防御的组成部分，不是
    // 事后挑数（门槛与种子池先验固定）。
    let mut plain: Vec<Net> = Vec::new();
    let mut member_report: Vec<Value> = Vec::new();
    for seed in 101u64..=130 {
        if plain.len() == 3 {
            break;
        }
        let net = train_fresh(seed, &features, &labels, &train, 16);
        let (_acc, val_f1) = evaluate(&net, &features, &labels, &val);
        let (_acc, test_f1) = evaluate(&net, &features, &labels, &test);
        let accepted = val_f1 >= 0.5;
        member_report.push(json!({
            "seed": seed,
            "val_f1": val_f1,
            "test_f1": test_f1,
            "accepted": accepted,
        }));
        if accepted {
            plain.push(net);
        }
    }

    // —— Part A：自适应探针 vs 固定探针（#495 方向修正的最后一步）。 ——
    // 攻击池：6 种训练内模式 × 6 实例；holdout：行交换(5)/行取反(6) 不进池。
    let pool_patterns = [0usize, 1, 2, 3, 4, 7];
    let holdout_patterns = [5usize, 6];
    let clean0 = &plain[0];
    let build_pool = |patterns: &[usize]| -> Vec<Net> {
        patterns
            .iter()
            .flat_map(|&p| {
                (0..6).map(move |inst| {
                    let mut prng = Rng::new(1000 + (p * 16 + inst) as u64);
                    apply_attack_pattern(clean0, p, &mut prng)
                })
            })
            .collect()
    };
    let pool = build_pool(&pool_patterns);
    let holdout = build_pool(&holdout_patterns);

    // 候选探针：200 个测试输入（shuffle 截取），签名 = 符号分歧。
    let mut cand = test.clone();
    let mut cand_rng = Rng::new(42);
    for i in (1..cand.len()).rev() {
        let j = cand_rng.next_index(i + 1);
        cand.swap(i, j);
    }
    cand.truncate(200);
    let probe_feats: Vec<Vec<f32>> = cand.iter().map(|&i| features[i].clone()).collect();
    let base_sig = probe_response(&plain[0], &probe_feats);
    let signature = |net: &Net| -> Vec<bool> {
        probe_response(net, &probe_feats)
            .iter()
            .zip(&base_sig)
            .map(|(a, b)| a != b)
            .collect()
    };
    let pool_sigs: Vec<Vec<bool>> = pool.iter().map(&signature).collect();
    let hold_sigs: Vec<Vec<bool>> = holdout.iter().map(&signature).collect();

    // 固定探针集：盲选（不看任何攻击样本）随机 50。
    let mut fixed_rng = Rng::new(999);
    let mut fixed_order: Vec<usize> = (0..probe_feats.len()).collect();
    for i in (1..fixed_order.len()).rev() {
        let j = fixed_rng.next_index(i + 1);
        fixed_order.swap(i, j);
    }
    fixed_order.truncate(PROBES);

    // 自适应探针集（防御侧 π_CP）：贪心最大新增覆盖，预算同为 50。
    // 签名 = 该输入上嫌疑工件的预测符号是否与基线分歧（标注免费）。
    let mut covered = vec![false; pool.len()];
    let mut adaptive_order: Vec<usize> = Vec::new();
    for _ in 0..PROBES {
        let mut best: Option<(usize, usize)> = None; // (新增覆盖, 候选下标)
        for (cand_i, _) in probe_feats.iter().enumerate() {
            if adaptive_order.contains(&cand_i) {
                continue;
            }
            let gain = pool_sigs
                .iter()
                .enumerate()
                .filter(|&(a, sig)| !covered[a] && sig[cand_i])
                .count();
            let better = match best {
                Some((g, _)) => gain > g,
                None => true,
            };
            if gain > 0 && better {
                best = Some((gain, cand_i));
            }
        }
        let Some((_gain, pick)) = best else { break };
        adaptive_order.push(pick);
        for (a, sig) in pool_sigs.iter().enumerate() {
            covered[a] |= sig[pick];
        }
    }

    let cov_at = |order: &[usize], k: usize, sigs: &[Vec<bool>]| -> f32 {
        let detected = sigs
            .iter()
            .filter(|sig| order[..k.min(order.len())].iter().any(|&p| sig[p]))
            .count();
        detected as f32 / sigs.len() as f32
    };
    let mut curves: Vec<Value> = Vec::new();
    for &k in &[1usize, 2, 4, 8, 16, 32, 50] {
        curves.push(json!({
            "k": k,
            "pool_adaptive": cov_at(&adaptive_order, k, &pool_sigs),
            "pool_fixed": cov_at(&fixed_order, k, &pool_sigs),
            "holdout_adaptive": cov_at(&adaptive_order, k, &hold_sigs),
            "holdout_fixed": cov_at(&fixed_order, k, &hold_sigs),
        }));
    }
    // 效率读数：自适应达到「固定集全预算覆盖率」需要几个探针。
    let fixed50 = cov_at(&fixed_order, PROBES, &pool_sigs);
    let k_match = (1..=adaptive_order.len())
        .find(|&k| cov_at(&adaptive_order, k, &pool_sigs) >= fixed50)
        .unwrap_or(adaptive_order.len());
    println!(
        "{}",
        json!({
            "stage": "partA_probes",
            "pool_nets": pool.len(),
            "holdout_nets": holdout.len(),
            "fixed50_pool_coverage": fixed50,
            "adaptive_probes_to_match": k_match,
            "adaptive_used": adaptive_order.len(),
            "curves": curves,
        })
    );

    // —— Part B：对抗集成（#499 成员进 #497 集成）——正交性定量验证。 ——
    // 红方：对 plain member 0 进化 kill-switch 基因组（4 代，k=16 行）。
    let attack_rows = evolve_attack(&plain[0], 16, &features, &labels, &test, &mut rng);

    // 蓝方对抗成员：从被攻击权重出发，RowDropout 轮换吃红方点名的行。
    let adv: Vec<Net> = plain
        .iter()
        .map(|net| {
            let mut member = net.clone();
            apply_rows(&mut member, &attack_rows);
            train_epochs(
                &mut member,
                &features,
                &labels,
                &train,
                8,
                &mut rng,
                &attack_rows,
            );
            member
        })
        .collect();

    let under_attack = |net: &Net| -> Net {
        let mut m = net.clone();
        apply_rows(&mut m, &attack_rows);
        m
    };
    let (_a, plain0_clean) = evaluate(&plain[0], &features, &labels, &test);
    let (_a, plain0_atk) = evaluate(&under_attack(&plain[0]), &features, &labels, &test);
    let (_a, adv0_clean) = evaluate(&adv[0], &features, &labels, &test);
    let (_a, adv0_atk) = evaluate(&under_attack(&adv[0]), &features, &labels, &test);

    let plain_atk: Vec<Net> = plain.iter().map(&under_attack).collect();
    let adv_atk: Vec<Net> = adv.iter().map(&under_attack).collect();
    let (_a, plain_ens_clean) = ensemble_evaluate(&plain, &features, &labels, &test);
    let (_a, plain_ens_atk) = ensemble_evaluate(&plain_atk, &features, &labels, &test);
    let (_a, adv_ens_clean) = ensemble_evaluate(&adv, &features, &labels, &test);
    let (_a, adv_ens_atk) = ensemble_evaluate(&adv_atk, &features, &labels, &test);
    let plain_members_atk: Vec<f32> = plain_atk
        .iter()
        .map(|m| evaluate(m, &features, &labels, &test).1)
        .collect();
    let adv_members_atk: Vec<f32> = adv_atk
        .iter()
        .map(|m| evaluate(m, &features, &labels, &test).1)
        .collect();

    // 对照：剔除输出行的同源攻击（只打 hidden 行）——区分「输出行单点故障」
    // 与「hidden 行跨成员迁移」两种失效机理。
    let hidden_rows: Vec<(usize, usize)> = attack_rows
        .iter()
        .filter(|&&(li, _)| li == 0)
        .copied()
        .collect();
    let under_hidden_attack = |net: &Net| -> Net {
        let mut m = net.clone();
        apply_rows(&mut m, &hidden_rows);
        m
    };
    let (_a, plain0_h_atk) = evaluate(&under_hidden_attack(&plain[0]), &features, &labels, &test);
    let (_a, adv0_h_atk) = evaluate(&under_hidden_attack(&adv[0]), &features, &labels, &test);
    let plain_h_atk: Vec<Net> = plain.iter().map(&under_hidden_attack).collect();
    let adv_h_atk: Vec<Net> = adv.iter().map(&under_hidden_attack).collect();
    let (_a, plain_ens_h_atk) = ensemble_evaluate(&plain_h_atk, &features, &labels, &test);
    let (_a, adv_ens_h_atk) = ensemble_evaluate(&adv_h_atk, &features, &labels, &test);

    println!(
        "{}",
        json!({
            "stage": "partB_ensemble",
            "attack_rows": attack_rows.len(),
            "attack_set": attack_rows
                .iter()
                .map(|(li, r)| format!("{li}:{r}"))
                .collect::<Vec<_>>(),
            "has_output_row": attack_rows.contains(&(1, 0)),
            "members": member_report,
            "single": {
                "plain0_clean_f1": plain0_clean,
                "plain0_attacked_f1": plain0_atk,
                "adv0_clean_f1": adv0_clean,
                "adv0_attacked_f1": adv0_atk,
            },
            "ensemble": {
                "plain_clean_f1": plain_ens_clean,
                "plain_attacked_f1": plain_ens_atk,
                "adv_clean_f1": adv_ens_clean,
                "adv_attacked_f1": adv_ens_atk,
            },
            "members_attacked_f1": {
                "plain": plain_members_atk,
                "adv": adv_members_atk,
            },
            "hidden_only_attack": {
                "rows": hidden_rows.len(),
                "plain0_attacked_f1": plain0_h_atk,
                "adv0_attacked_f1": adv0_h_atk,
                "plain_ensemble_attacked_f1": plain_ens_h_atk,
                "adv_ensemble_attacked_f1": adv_ens_h_atk,
            },
            "orthogonality": {
                "gain_adv_single": adv0_atk - plain0_atk,
                "gain_ensemble_plain": plain_ens_atk - plain0_atk,
                "gain_combined": adv_ens_atk - plain0_atk,
            },
        })
    );
}
