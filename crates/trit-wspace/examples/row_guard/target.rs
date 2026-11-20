//! 夹具工件与 K 行读数：一次 #492 口径训练（本实验唯一的训练），其余全编辑操作。

use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::{Layer, Net};

use super::metrics::{FEAT_DIM, HIDDEN, Rng};

/// 正常初始化的完整 QAT 训练（#492 口径：latent ±0.05 + 行 absmean scale）。
/// 只做一次当夹具；replica 构建与全部攻击都是编辑/构建操作，零训练。
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

/// 输出层完整向量：K 行副本 → K 个 logit（经 tanh）。守卫语义的读数基础。
pub(super) fn infer_rows(net: &Net, x: &[f32]) -> Vec<f32> {
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
    v
}

/// 输出层 pre-activation 向量（scale×acc，不过 tanh）。告警必须读这一层：
/// hinge 训练把后激活推到 tanh 完全饱和（±1.0f32），tanh(3x)=tanh(x) 逐位
/// 相同——幅度类篡改（scale×3）在后激活上不可见（首轮实测告警 0.0）。
pub(super) fn infer_logits(net: &Net, x: &[f32]) -> Vec<f32> {
    let layers = net.layers();
    let mut v = x.to_vec();
    for (li, layer) in layers.iter().enumerate() {
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
        if li + 1 == layers.len() {
            return next;
        }
        v = next.iter().map(|&x| Activation::Tanh.forward(x)).collect();
    }
    v
}

/// 标量读数（v[0]）——replica 的 row 0 与原工件逐位同，语义 drop-in 兼容。
pub(super) fn infer(net: &Net, x: &[f32]) -> f32 {
    infer_rows(net, x)[0]
}

/// 标量评测：(acc, spam-F1)。
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

/// K 行投票：被置零行弃权（logit=0），存活行决定预测。副本逐位相同 →
/// 任一存活行即全体决策，不存在分票。读 pre-activation（tanh 单调，符号同）。
fn row_vote(net: &Net, x: &[f32]) -> f32 {
    for &v in &infer_logits(net, x) {
        if v != 0.0 {
            return v;
        }
    }
    0.0
}

/// 投票评测：(acc, spam-F1)。
pub(super) fn evaluate_vote(
    net: &Net,
    features: &[Vec<f32>],
    labels: &[usize],
    test: &[usize],
) -> (f32, f32) {
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_ = 0usize;
    for &i in test {
        let pred = usize::from(row_vote(net, &features[i]) > 0.0);
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
