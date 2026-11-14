//! 蓝方防御工件：QAT 训练、推理与红队攻击执行（从 main 拆出）。

use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

use super::metrics::{FEAT_DIM, HIDDEN, Rng};

/// 蓝方防御工件：一个集成成员（单层输出判别器）。
pub(super) struct Defender {
    pub(super) net: Net,
}

/// QAT 训练一个集成成员（absmean scale 初始化，hinge，Adam×2）。
pub(super) fn train_defender(
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
    }
}

/// 单个防御成员的输出（评估成员被 kill-switch 后的退化）。
pub(super) fn member_out(d: &Defender, x: &[f32]) -> f32 {
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
pub(super) fn apply_genome_attack(defender: &mut Defender, rows: &[(usize, usize)]) {
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
