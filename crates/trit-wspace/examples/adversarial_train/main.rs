//! 对抗训练：让蓝方**吃进化攻击样本**——行级置零注入（RowDropout）。
//!
//! #494/#497 的结论：SMS spam 判别式工件存在单点支配行，进化攻击 gen 0 就
//! 能找到 kill-switch（整行置零 → F1 0.75→0.00），集成投票可扛但单模型无解。
//! 本例实现 #497 列的防御对策——**对抗训练**：
//!
//! - **红方**：进化引擎找对当前蓝方最强攻击的行组合（基因 = 行优先级三值
//!   向量，攻击 = 整行置零）；
//! - **蓝方**：对抗再训练——每个 epoch 以概率 p 把红方点名的行**置零后
//!   训练**（RowDropout 注入，epoch 末恢复），强迫决策边界离开单行；
//! - **AB 循环**：红蓝交替 R 轮，读数 = 每轮攻击打击量递减 + 干净 F1 税 +
//!   对照（普通再训练，无注入）。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example adversarial_train -- \
//!   --file <数据盘>/trit-data/sms-spam/SMSSpamCollection --rounds 3
//! ```

mod attacker;
mod metrics;

use attacker::evolve_attack;
use metrics::{Rng, featurize, load_dataset, stratified_split};
use serde_json::{Value, json};
use trit_learn::dense::TernaryDense;

use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

const HIDDEN: usize = 128;

/// QAT 推理（量化口径）：y = scale·(t·x)，层间 Tanh。
fn infer(net: &Net, x: &[f32]) -> f32 {
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

pub(crate) fn evaluate(
    net: &Net,
    features: &[Vec<f32>],
    labels: &[usize],
    test: &[usize],
) -> (f32, f32) {
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_ = 0usize;
    for &i in test {
        let pred = usize::from(infer(net, &features[i]) > 0.0);
        match (pred, labels[i]) {
            (1, 1) => tp += 1,
            (1, 0) => fp += 1,
            (0, 1) => fn_ += 1,
            _ => {}
        }
    }
    let precision = tp as f32 / (tp + fp).max(1) as f32;
    let recall = tp as f32 / (tp + fn_).max(1) as f32;
    let f1 = 2.0 * precision * recall / (precision + recall).max(1e-12);
    let acc = (tp + fn_) as f32 / (tp + fp + fn_ + test.len().saturating_sub(tp + fp + fn_)) as f32;
    (acc, f1)
}

/// 从工件反推 QAT latent（再训练从现有权重出发）。
fn latent_from_net(net: &Net) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let pull = |layer: &Layer| -> Vec<Vec<f32>> {
        let cols = layer.cols();
        (0..layer.rows())
            .map(|row| {
                (0..cols)
                    .map(|c| f32::from(layer.trits()[row * cols + c].to_i8()) * layer.scales()[row])
                    .collect()
            })
            .collect()
    };
    (pull(&net.layers()[0]), pull(&net.layers()[1]))
}

fn write_back(net: &mut Net, hidden: &TernaryDense, out: &TernaryDense) {
    // Net/Layer 无 mut 访问器——写回走正规编辑原语（攻击者与防御者同一入口）。
    let mut edits = Vec::new();
    for (li, dense) in [hidden, out].iter().enumerate() {
        for row in 0..dense.rows() {
            let q = dense.quantized_row(row);
            for (c, &t) in q.iter().enumerate() {
                let cur = net.layers()[li].trits()[row * dense.cols() + c].to_i8();
                if cur != t {
                    edits.push(Edit::SetTrit {
                        layer: li,
                        row,
                        col: c,
                        to: Trit::from_i8(t).expect("valid"),
                    });
                }
            }
            if (net.layers()[li].scales()[row] - dense.scales()[row]).abs() > f32::EPSILON {
                edits.push(Edit::SetScale {
                    layer: li,
                    row,
                    to: dense.scales()[row],
                });
            }
        }
    }
    Patch::new(edits).apply(net).expect("in-bounds");
}

/// 单 epoch QAT 训练（从 net 出发写回 net）；`zero_row` = 本 epoch 置零的
/// hidden 行（RowDropout 注入，None 表示无注入）。
fn train_one_epoch(
    net: &mut Net,
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    rng: &mut Rng,
    zero_row: Option<usize>,
) -> f32 {
    let (mut hidden_latent, out_latent) = latent_from_net(net);
    if let Some(r) = zero_row {
        hidden_latent[r].fill(0.0);
    }
    let make = |latent: &[Vec<f32>]| -> TernaryDense {
        let rows = latent.len();
        let cols = latent[0].len();
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
    let mut hidden = make(&hidden_latent);
    let mut out = make(&out_latent);
    let mut adam_hl = Adam::new(1e-3);
    let mut adam_hs = Adam::new(1e-3);
    let mut adam_ol = Adam::new(1e-3);
    let mut adam_os = Adam::new(1e-3);
    let mut order = train.to_vec();
    for i in (1..order.len()).rev() {
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        order.swap(i, j);
    }
    let mut total_loss = 0.0f32;
    for &index in &order {
        let target = if labels[index] == 1 { 1.0 } else { -1.0 };
        let h = hidden.forward(&features[index]);
        let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
        let o = out.forward(&h_act);
        let o_act: Vec<f32> = o.iter().map(|&v| Activation::Tanh.forward(v)).collect();
        let (loss, grad_o_act) = hinge(&o_act, &[target]);
        total_loss += loss;
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
    write_back(net, &hidden, &out);
    total_loss / order.len() as f32
}

/// 多 epoch 训练；`row_dropout_p` = 每 epoch 注入随机行置零的概率。
fn train_epochs(
    net: &mut Net,
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    epochs: usize,
    rng: &mut Rng,
    focus_rows: &[(usize, usize)],
) {
    // RowDropout 注入：轮换红方点名的行（没有点名行时退化为随机行——
    // 普通再训练对照就用空表 + 随机）。
    // 注入只在 hidden 行（输出层行注入跳过——其置零等价于分类器熄火，
    // 不是「行冗余」要治的对象）。
    let hidden_focus: Vec<usize> = focus_rows
        .iter()
        .filter(|&&(li, _)| li == 0)
        .map(|&(_, row)| row)
        .collect();
    for e in 0..epochs {
        let zero_row = if hidden_focus.is_empty() {
            if focus_rows.is_empty() {
                Some(rng.next_index(HIDDEN))
            } else {
                None
            }
        } else {
            Some(hidden_focus[e % hidden_focus.len()])
        };
        train_one_epoch(net, features, labels, train, rng, zero_row);
    }
}

fn apply_rows(net: &mut Net, rows: &[(usize, usize)]) {
    let mut edits = Vec::new();
    for &(li, row) in rows {
        let cols = net.layers()[li].cols();
        for c in 0..cols {
            if net.layers()[li].trits()[row * cols + c].to_i8() != 0 {
                edits.push(Edit::SetTrit {
                    layer: li,
                    row,
                    col: c,
                    to: Trit::Zero,
                });
            }
        }
    }
    Patch::new(edits).apply(net).expect("in-bounds");
}

/// 正常初始化的完整 QAT 训练（#492 口径：latent ±0.05 + 行 absmean scale）。
/// **不能用全零工件反推**——对称死区里 Adam 打破不了行内对称（实测 F1 恒 0）。
fn train_fresh(
    seed: u64,
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    epochs: usize,
) -> Net {
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
    let mut hidden = make(HIDDEN, features[0].len(), &mut rng);
    let mut out = make(1, HIDDEN, &mut rng);
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
            let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            let o = out.forward(&h_act);
            let o_act: Vec<f32> = o.iter().map(|&v| Activation::Tanh.forward(v)).collect();
            let (_loss, grad_o_act) = hinge(&o_act, &[target]);
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
    Net::new(vec![build(&hidden), build(&out)]).expect("chainable")
}

fn main() {
    let mut file = String::from("D:/KF/trit-data/sms-spam/SMSSpamCollection");
    let mut rounds = 3usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--file" => file = argv.next().expect("file"),
            "--rounds" => rounds = argv.next().expect("num").parse().expect("num"),
            other => panic!("未知参数 {other}"),
        }
    }

    let dataset = load_dataset(&file);
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, _val, test) = stratified_split(&labels, &mut rng);

    // 基线模型（普通 QAT，无对抗注入）。
    // 基线：纯 QAT 训练（train_one_epoch 的 zero_row 传 None 的包装）。
    let mut model = Model {
        net: train_fresh(100, &features, &labels, &train, 16),
    };
    let (base_acc, base_f1) = evaluate(&model.net, &features, &labels, &test);
    println!(
        "{}",
        json!({"stage": "baseline", "acc": base_acc, "f1": base_f1})
    );

    // 对照组：普通再训练副本（无 RowDropout 注入）。
    let mut plain = Model {
        net: model.net.clone(),
    };

    let mut history: Vec<Value> = Vec::new();
    for round in 0..rounds {
        // 红方：进化找对当前蓝方最强攻击。
        let attack_rows = evolve_attack(&model.net, 16, &features, &labels, &test, &mut rng, 4);
        let mut attacked = Model {
            net: model.net.clone(),
        };
        apply_rows(&mut attacked.net, &attack_rows);
        let (_, attacked_f1) = evaluate(&attacked.net, &features, &labels, &test);

        // 蓝方 A（对抗再训练）：从被攻击权重出发，**吃红方点名的攻击行**
        // （RowDropout 轮换注入——这就是「吃进化攻击样本」）。
        let mut adv = Model {
            net: attacked.net.clone(),
        };
        train_epochs(
            &mut adv.net,
            &features,
            &labels,
            &train,
            8,
            &mut rng,
            &attack_rows,
        );
        let (adv_acc, adv_f1) = evaluate(&adv.net, &features, &labels, &test);
        let mut adv_attacked = Model {
            net: adv.net.clone(),
        };
        apply_rows(&mut adv_attacked.net, &attack_rows);
        let (_, adv_re_f1) = evaluate(&adv_attacked.net, &features, &labels, &test);

        // 蓝方 B（对照）：普通再训练（无 RowDropout 注入），同源攻击再打。
        train_epochs(&mut plain.net, &features, &labels, &train, 8, &mut rng, &[]);
        let mut plain_attacked = Model {
            net: plain.net.clone(),
        };
        apply_rows(&mut plain_attacked.net, &attack_rows);
        let (_pa, plain_re_f1) = evaluate(&plain_attacked.net, &features, &labels, &test);

        model = adv.clone();
        history.push(json!({
            "round": round + 1,
            "attacked_f1": attacked_f1,
            "adv_clean_f1": adv_f1,
            "adv_clean_acc": adv_acc,
            "adv_under_same_attack_f1": adv_re_f1,
            "plain_retrain_under_same_attack_f1": plain_re_f1,
        }));
        println!(
            "{}",
            json!({
                "stage": "round",
                "round": round + 1,
                "attack_rows": attack_rows.len(),
                "attacked_f1": attacked_f1,
                "adv_clean_f1": adv_f1,
                "adv_under_attack_f1": adv_re_f1,
                "plain_retrain_under_attack_f1": plain_re_f1,
            })
        );
    }
    let (final_acc, final_f1) = evaluate(&model.net, &features, &labels, &test);
    println!(
        "{}",
        json!({
            "stage": "summary",
            "rounds": rounds,
            "baseline_f1": base_f1,
            "final_clean_f1": final_f1,
            "final_clean_acc": final_acc,
            "history": history,
        })
    );
}

#[derive(Clone)]
struct Model {
    net: Net,
}
