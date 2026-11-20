//! 目标工件与共享读数：普通 QAT 训练、冻结推理、F1 评测（#492/#499 口径）。

use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::{Layer, Net};

use super::metrics::{FEAT_DIM, HIDDEN, Rng};

/// 正常初始化的完整 QAT 训练（#492 口径：latent ±0.05 + 行 absmean scale）。
/// **不能用全零工件反推**——对称死区里 Adam 打破不了行内对称（实测 F1 恒 0）。
pub(super) fn train_fresh(
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
    let mut hidden = make(HIDDEN, FEAT_DIM, &mut rng);
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
    Net::new(vec![build(&hidden), build(&out)]).expect("chainable")
}

/// 冻结工件推理（标量输出）。
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

/// 测试集评测：(acc, spam-F1)。
pub(super) fn evaluate(
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
