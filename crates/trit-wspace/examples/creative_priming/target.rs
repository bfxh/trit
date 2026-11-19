//! 冻结执行器与共享读数（从 main 拆出，守模块预算）。

use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

use super::metrics::{FEAT_DIM, HIDDEN, Rng};

pub(super) const GENOME_LEN: usize = 129; // 128 hidden 行 + 1 输出行（#494 kill-switch 所在）

/// 冻结的攻击执行器目标：SMS spam 防御工件（#492 口径训练）。
pub(super) struct Target {
    pub(super) net: Net,
}

pub(super) fn train_target(
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    seed: u64,
) -> Target {
    let mut rng = Rng::new(seed);
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
    let mut hidden = make(HIDDEN, FEAT_DIM, &mut rng);
    let mut out = make(1, HIDDEN, &mut rng);
    let mut adam_hl = Adam::new(1e-3);
    let mut adam_hs = Adam::new(1e-3);
    let mut adam_ol = Adam::new(1e-3);
    let mut adam_os = Adam::new(1e-3);
    for _ in 0..20 {
        let mut order = train.to_vec();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        for &index in &order {
            let target_v = if labels[index] == 1 { 1.0 } else { -1.0 };
            let h = hidden.forward(&features[index]);
            let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            let o = out.forward(&h_act);
            let o_act: Vec<f32> = o.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            let (_loss, grad_o_act) = hinge(&o_act, &[target_v]);
            let grad_o: Vec<f32> = grad_o_act
                .iter()
                .zip(&o_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let grad_h_act = out.backward(&grad_o);
            out.update(&mut adam_ol, &mut adam_os);
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_hl, &mut adam_hs);
        }
    }
    let build = |dense: &TernaryDense| -> Layer {
        let trits: Vec<i8> = dense.quantized().into_iter().flatten().collect();
        Layer::with_scales(
            dense.rows(),
            dense.cols(),
            &trits,
            &vec![0i8; dense.rows()],
            dense.scales().to_vec(),
        )
        .expect("valid")
    };
    Target {
        net: Net::new(vec![build(&hidden), build(&out)]).expect("chainable"),
    }
}

/// 冻结执行器：把 primer（三值行优先级基因组）翻译成 `Edit::SetTrit` 攻击
/// ——+1 行整行置零，-1 行整行取反（新增的攻击动词），0 不动。
/// 执行器本身不可训练（π_resp 冻结语义）：攻击动词表是固定的。
pub(super) fn execute_primer(target: &mut Target, primer: &[i8]) -> usize {
    let mut edits = Vec::new();
    for (row, &prio) in primer.iter().enumerate() {
        let (li, r) = if row < HIDDEN {
            (0usize, row)
        } else {
            (1usize, 0usize)
        };
        let cols = target.net.layers()[li].cols();
        match prio {
            1 => {
                for c in 0..cols {
                    if target.net.layers()[li].trits()[r * cols + c].to_i8() != 0 {
                        edits.push(Edit::SetTrit {
                            layer: li,
                            row: r,
                            col: c,
                            to: Trit::Zero,
                        });
                    }
                }
            }
            -1 => {
                for c in 0..cols {
                    let cur = target.net.layers()[li].trits()[r * cols + c].to_i8();
                    if cur != 0 {
                        edits.push(Edit::SetTrit {
                            layer: li,
                            row: r,
                            col: c,
                            to: Trit::from_i8(-cur).expect("valid"),
                        });
                    }
                }
            }
            _ => {}
        }
    }
    let n = edits.len();
    Patch::new(edits).apply(&mut target.net).expect("in-bounds");
    n
}

/// 冻结工件推理。
pub(super) fn infer(net: &Net, x: &[f32]) -> f32 {
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
}

/// 攻击后任务质量：spam F1 的下降量（质量项）。
pub(super) fn attack_quality(
    target: &Target,
    clean_net: &Net,
    features: &[Vec<f32>],
    labels: &[usize],
    test: &[usize],
) -> f32 {
    let mut confusion_clean = trit_learn::metrics::Confusion::new(2);
    let mut confusion_atk = trit_learn::metrics::Confusion::new(2);
    for &i in test {
        let pred_clean = usize::from(infer(clean_net, &features[i]) > 0.0);
        let pred_atk = usize::from(infer(&target.net, &features[i]) > 0.0);
        confusion_clean.observe(pred_clean, labels[i]);
        confusion_atk.observe(pred_atk, labels[i]);
    }
    let per_clean = confusion_clean.per_class();
    let per_atk = confusion_atk.per_class();
    let f1 = |p: &(f32, f32, f32)| p.2;
    (f1(&per_clean[1]) - f1(&per_atk[1])).abs()
}

/// 三值基因组的两两平均汉明距离（多样性主指标）。
pub(super) fn mean_hamming(genomes: &[Vec<i8>]) -> f32 {
    if genomes.len() < 2 {
        return 0.0;
    }
    let mut total = 0.0f32;
    let mut pairs = 0usize;
    for i in 0..genomes.len() {
        for j in (i + 1)..genomes.len() {
            let d = genomes[i]
                .iter()
                .zip(genomes[j].iter())
                .filter(|(a, b)| a != b)
                .count() as f32;
            total += d / genomes[i].len() as f32;
            pairs += 1;
        }
    }
    total / pairs.max(1) as f32
}

/// 行覆盖熵：primer 群体在 129 行上的选择分布熵（0=全部挤在同一行=深井）。
pub(super) fn coverage_entropy(genomes: &[Vec<i8>]) -> f32 {
    let mut counts = vec![0.0f32; GENOME_LEN];
    for g in genomes {
        for (row, &prio) in g.iter().enumerate() {
            if prio != 0 {
                counts[row] += 1.0;
            }
        }
    }
    let total: f32 = counts.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    -counts
        .iter()
        .filter(|&&c| c > 0.0)
        .map(|&c| {
            let p = c / total;
            p * p.log2()
        })
        .sum::<f32>()
}

pub(super) fn median(values: &mut [f32]) -> f32 {
    values.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
    values[values.len() / 2]
}
