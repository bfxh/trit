//! 三值 QAT 训练 + 工件红队：UCI SMS Spam（真实数据集，白客防御任务）全链。
//! 全部 Rust（训练禁 Python 硬门一致）。管线六步：数据（SMS Spam 5,574 条，
//! `docs/DATASETS.md` A0）→ 特征（自造：字符 3/4-gram FNV 哈希 4096 维，
//! 出现=+1，输入本身三值，无预训练 embedding）→ 模型（QAT `TernaryDense`
//! 4096→128→1，`Ste::Clipped`，Tanh/Tanh，hinge）→ 评测（自造：分层
//! 70/15/15，acc/P/R/F1 + 混淆矩阵）→ 导出（量化权重 → `trit-wspace` Layer
//! → WSP2 工件）→ 红队（接 #489：真实任务指标下的比特翻转攻击）。读数与坑
//! 见 `docs/WSP-SPAM-QAT.md`。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example spam_redteam -- \
//!   --file <数据盘>/trit-data/sms-spam/SMSSpamCollection
//! ```

mod metrics;

use metrics::{Rng, evaluate, featurize, load_dataset, qat_out, stratified_split};
use serde_json::{Value, json};
use trit_learn::Activation;
use trit_learn::Ste;
use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_wspace::edit::{Edit, Patch};
use trit_wspace::trit::Trit;
use trit_wspace::{Layer, Net};

const FEAT_DIM: usize = 4096;
const HIDDEN: usize = 128;

fn main() {
    let mut file = String::from("D:/KF/trit-data/sms-spam/SMSSpamCollection");
    let mut epochs = 30usize;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--file" => file = argv.next().expect("file"),
            "--epochs" => epochs = argv.next().expect("num").parse().expect("num"),
            other => panic!("未知参数 {other}"),
        }
    }

    // 1) 数据。
    let dataset = load_dataset(&file);
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, val, test) = stratified_split(&labels, &mut rng);
    println!(
        "{}",
        json!({
            "stage": "data",
            "total": dataset.len(),
            "spam": labels.iter().filter(|&&l| l == 1).count(),
            "train": train.len(),
            "val": val.len(),
            "test": test.len(),
        })
    );

    // 2) 模型：4096→128(Tanh)→1(Tanh)，QAT + STE。
    let ste = Ste::Clipped { ratio: 1.0 };
    // 初始化：小尺度均匀 latent；scale0 取**行 absmean**（与 `Layer::from_f32`
    // 同口径）——否则 latent=±0.05、scale=1 时整层量化成 0，网络是死的。
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
    let mut adam_h_latent = Adam::new(1e-3);
    let mut adam_h_scale = Adam::new(1e-3);
    let mut adam_o_latent = Adam::new(1e-3);
    let mut adam_o_scale = Adam::new(1e-3);

    // 3) 训练（单样本 SGD，逐 epoch 洗牌）。
    for epoch in 0..epochs {
        let mut order = train.clone();
        for i in (1..order.len()).rev() {
            let j = (rng.next_u64() % (i as u64 + 1)) as usize;
            order.swap(i, j);
        }
        let mut total_loss = 0.0f32;
        for &index in &order {
            let target = if labels[index] == 1 { 1.0 } else { -1.0 };
            let h = hidden.forward(&features[index]);
            let h_act: Vec<f32> = h.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let o = out_layer.forward(&h_act);
            let o_act: Vec<f32> = o.iter().map(|&x| Activation::Tanh.forward(x)).collect();
            let (loss, grad_o_act) = hinge(&o_act, &[target]);
            total_loss += loss;
            // 输出层反向。
            let grad_o: Vec<f32> = grad_o_act
                .iter()
                .zip(&o_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let grad_h_act = out_layer.backward(&grad_o);
            out_layer.update(&mut adam_o_latent, &mut adam_o_scale);
            // 隐层反向。
            let grad_h_pre: Vec<f32> = grad_h_act
                .iter()
                .zip(&h_act)
                .map(|(&g, &y)| g * Activation::Tanh.backward(y))
                .collect();
            let _ = hidden.backward(&grad_h_pre);
            hidden.update(&mut adam_h_latent, &mut adam_h_scale);
        }
        if epoch % 5 == 4 || epoch + 1 == epochs {
            let val_outs: Vec<f32> = val
                .iter()
                .map(|&i| qat_out(&mut hidden, &mut out_layer, &features, i))
                .collect();
            let metrics = evaluate(
                &val_outs,
                &val.iter().map(|&i| labels[i]).collect::<Vec<_>>(),
            );
            println!(
                "{}",
                json!({"stage": "train", "epoch": epoch + 1, "loss": total_loss / order.len() as f32, "val": metrics.to_value()})
            );
        }
    }

    // 4) 测试集读数（QAT 模型）。
    let test_outs: Vec<f32> = test
        .iter()
        .map(|&i| qat_out(&mut hidden, &mut out_layer, &features, i))
        .collect();
    let test_labels: Vec<usize> = test.iter().map(|&i| labels[i]).collect();
    let clean_metrics = evaluate(&test_outs, &test_labels);
    println!(
        "{}",
        json!({"stage": "test_qat", "metrics": clean_metrics.to_value()})
    );

    // 5) 导出为 trit-wspace 工件（量化权重 + scale，bias=0）。
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
    // 量化推理口径（工件推理）：y = scale·(t·x)，输出取 tanh。
    let artifact_infer = |net: &Net, x: &[f32]| -> f32 {
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
    };
    let artifact =
        Net::new(vec![build_layer(&hidden), build_layer(&out_layer)]).expect("chainable");
    let artifact_outs: Vec<f32> = test
        .iter()
        .map(|&i| artifact_infer(&artifact, &features[i]))
        .collect();
    let artifact_metrics = evaluate(&artifact_outs, &test_labels);
    println!(
        "{}",
        json!({"stage": "test_artifact", "metrics": artifact_metrics.to_value()})
    );

    // 6) 红队：翻转 k 个 trit（随机，5 次重复）后 spam F1/漏检率。
    let total_trits: usize = artifact.layers().iter().map(|l| l.rows() * l.cols()).sum();
    let mut attack_report: Vec<Value> = Vec::new();
    for &k in &[1_000usize, 10_000, 100_000, 500_000] {
        let mut f1s = Vec::new();
        let mut misses = Vec::new();
        for _ in 0..5 {
            let mut attacked = artifact.clone();
            for _ in 0..k {
                let li = (rng.next_u64() % attacked.layers().len() as u64) as usize;
                let layer = &attacked.layers()[li];
                let row = (rng.next_u64() % layer.rows() as u64) as usize;
                let col = (rng.next_u64() % layer.cols() as u64) as usize;
                let current = layer.trits()[row * layer.cols() + col].to_i8();
                let to = match current {
                    -1 => 0,
                    0 => 1,
                    _ => 0,
                };
                Patch::new(vec![Edit::SetTrit {
                    layer: li,
                    row,
                    col,
                    to: Trit::from_i8(to).expect("valid"),
                }])
                .apply(&mut attacked)
                .expect("in-bounds");
            }
            let outs: Vec<f32> = test
                .iter()
                .map(|&i| artifact_infer(&attacked, &features[i]))
                .collect();
            let metrics = evaluate(&outs, &test_labels);
            f1s.push(metrics.f1());
            // 漏检率：spam 被判成 ham 的比例。
            misses.push(metrics.fn_ as f32 / (metrics.fn_ + metrics.tp).max(1) as f32);
        }
        f1s.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        misses.sort_by(|a, b| a.partial_cmp(b).expect("finite"));
        attack_report.push(json!({
            "k": k,
            "f1_median": f1s[f1s.len() / 2],
            "spam_miss_rate_median": misses[misses.len() / 2],
        }));
    }
    println!(
        "{}",
        json!({
            "stage": "redteam",
            "total_trits": total_trits,
            "clean_f1": clean_metrics.f1(),
            "attacks": attack_report,
        })
    );
}
