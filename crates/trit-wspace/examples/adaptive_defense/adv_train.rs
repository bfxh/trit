//! RowDropout 对抗再训练（#499 口径）：从被攻击权重出发，轮换注入红方点名的行。

use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

use super::metrics::{HIDDEN, Rng};

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

/// 把训练后的 latent 写回工件——Net/Layer 无 mut 访问器，走正规编辑原语
/// （攻击者与防御者同一入口）。
fn write_back(net: &mut Net, hidden: &TernaryDense, out: &TernaryDense) {
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
        let target_v = if labels[index] == 1 { 1.0 } else { -1.0 };
        let h = hidden.forward(&features[index]);
        let h_act: Vec<f32> = h.iter().map(|&v| Activation::Tanh.forward(v)).collect();
        let o = out.forward(&h_act);
        let o_act: Vec<f32> = o.iter().map(|&v| Activation::Tanh.forward(v)).collect();
        let (loss, grad_o_act) = hinge(&o_act, &[target_v]);
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

/// 多 epoch 训练；轮换注入红方点名的行（没有点名行时退化为随机行）。
/// 注入只在 hidden 行：输出行置零等价于分类器熄火，不是「行冗余」要治的对象。
pub(super) fn train_epochs(
    net: &mut Net,
    features: &[Vec<f32>],
    labels: &[usize],
    train: &[usize],
    epochs: usize,
    rng: &mut Rng,
    focus_rows: &[(usize, usize)],
) {
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

/// 对工件施加行级置零攻击（kill-switch 家族，#494 口径）。
pub(super) fn apply_rows(net: &mut Net, rows: &[(usize, usize)]) {
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
