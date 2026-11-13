//! 红队第三弹：把攻击者从「代理启发式」升级到**实测定位**——probe 口径的
//! 实测消融：对工件每行把 trits 全部置零（`Edit::SetTrit` 批量 Patch），量
//! 输出偏移，得到攻击者可实测（无需梯度）的行级承重表。目标：SMS spam
//! 分类器（#492 工件，F1 0.75 口径）。读数：三档攻击者（随机 / #489 行方差
//! 代理 / 实测行级）+ kill-switch 单行摧毁，见 `docs/WSP-REDTEAM-PROBE.md`。
//!
//! ```bash
//! cargo run --release -p trit-wspace --example redteam_probe -- \
//!   --file <数据盘>/trit-data/sms-spam/SMSSpamCollection
//! ```

mod metrics;

use metrics::{Rng, artifact_out, evaluate, featurize, load_dataset, median, stratified_split};
use serde_json::{Value, json};
use trit_learn::dense::TernaryDense;
use trit_learn::loss::hinge;
use trit_learn::opt::Adam;
use trit_learn::{Activation, Ste};
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

    // 1) 数据 + 训练（同 #492 配置的快速版）。
    let dataset = load_dataset(&file);
    let labels: Vec<usize> = dataset.iter().map(|(l, _)| *l).collect();
    let features: Vec<Vec<f32>> = dataset.iter().map(|(_, t)| featurize(t)).collect();
    let mut rng = Rng::new(7);
    let (train, _val, test) = stratified_split(&labels, &mut rng);
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
    for epoch in 0..epochs {
        let mut order = train.clone();
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
        println!("{}", json!({"stage": "train", "epoch": epoch + 1}));
    }

    // 2) 导出工件 + 测试基线。
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
    let artifact =
        Net::new(vec![build_layer(&hidden), build_layer(&out_layer)]).expect("chainable");
    let test_labels: Vec<usize> = test.iter().map(|&i| labels[i]).collect();
    let clean_outs: Vec<f32> = test
        .iter()
        .map(|&i| artifact_out(&artifact, &features[i]))
        .collect();
    let clean = evaluate(&clean_outs, &test_labels);
    println!(
        "{}",
        json!({"stage": "test_artifact", "acc": clean.accuracy(), "f1": clean.f1(), "recall": clean.recall()})
    );

    // 3) 实测行级重要性（probe 消融口径）：把行 trits 全置零，量测试子集
    //    （前 64 个样本）输出平均 |Δy|。攻击者无需梯度，只需推理接口。
    let probe_n = 64.min(test.len());
    let probe_rows: Vec<usize> = test[..probe_n].to_vec();
    let mut measured: Vec<(f32, usize, usize)> = Vec::new(); // (|Δy|, layer, row)
    for (li, layer) in artifact.layers().iter().enumerate() {
        for row in 0..layer.rows() {
            let edits: Vec<Edit> = (0..layer.cols())
                .filter(|&c| layer.trits()[row * layer.cols() + c].to_i8() != 0)
                .map(|c| Edit::SetTrit {
                    layer: li,
                    row,
                    col: c,
                    to: Trit::Zero,
                })
                .collect();
            if edits.is_empty() {
                continue;
            }
            let mut ablated = artifact.clone();
            Patch::new(edits).apply(&mut ablated).expect("in-bounds");
            let mut delta = 0.0f32;
            for &i in &probe_rows {
                let base = artifact_out(&artifact, &features[i]);
                let ablated_out = artifact_out(&ablated, &features[i]);
                delta += (base - ablated_out).abs();
            }
            measured.push((delta / probe_n as f32, li, row));
        }
    }
    measured.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));
    println!(
        "{}",
        json!({
            "stage": "probe",
            "rows_ranked": measured.len(),
            "top5_delta": measured.iter().take(5).map(|(d, _, _)| *d).collect::<Vec<_>>(),
        })
    );

    // 4) 三档攻击对比：k ∈ {10, 100, 1000}，5 次重复取中位 F1。
    //    行方差代理（#489）与实测行级都是「top-k 行各翻 1 个 trit」。
    let mut rows_by_var: Vec<(f32, usize, usize)> = artifact
        .layers()
        .iter()
        .enumerate()
        .flat_map(|(li, layer)| {
            layer.scales().iter().enumerate().map(move |(row, &s)| {
                let nnz = layer.trits()[row * layer.cols()..(row + 1) * layer.cols()]
                    .iter()
                    .filter(|t| t.to_i8() != 0)
                    .count() as f32;
                (s * s * nnz, li, row)
            })
        })
        .collect();
    rows_by_var.sort_by(|a, b| b.0.partial_cmp(&a.0).expect("finite"));
    let top_of = |rows: &[(f32, usize, usize)], k: usize| -> Vec<(usize, usize)> {
        rows.iter().take(k).map(|&(_, li, row)| (li, row)).collect()
    };
    let measured_rows = top_of(&measured, 1000);
    let var_rows = top_of(&rows_by_var, 1000);
    let mut report: Vec<Value> = Vec::new();
    for &k in &[10usize, 100, 1000] {
        let run_attacks = |targets: &[(usize, usize)]| -> (f32, f32) {
            let mut f1s = Vec::new();
            let mut accs = Vec::new();
            for _ in 0..5 {
                let mut attacked = artifact.clone();
                for &(li, row) in targets {
                    let layer = &attacked.layers()[li];
                    let cols = layer.cols();
                    let col = (0..cols)
                        .find(|&c| layer.trits()[row * cols + c].to_i8() != 0)
                        .unwrap_or(0);
                    let current = layer.trits()[row * cols + col].to_i8();
                    let to = if current == 0 { 1 } else { 0 };
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
                    .map(|&i| artifact_out(&attacked, &features[i]))
                    .collect();
                let m = evaluate(&outs, &test_labels);
                f1s.push(m.f1());
                accs.push(m.accuracy());
            }
            (median(&mut f1s), median(&mut accs))
        };
        // 随机：k 个随机 (layer,row)。
        let random_targets: Vec<(usize, usize)> = (0..k)
            .map(|_| {
                let li = (rng.next_u64() % artifact.layers().len() as u64) as usize;
                let row = (rng.next_u64() % artifact.layers()[li].rows() as u64) as usize;
                (li, row)
            })
            .collect();
        let (rand_f1, rand_acc) = run_attacks(&random_targets);
        let (var_f1, var_acc) = run_attacks(&var_rows[..k.min(var_rows.len())]);
        let (meas_f1, meas_acc) = run_attacks(&measured_rows[..k.min(measured_rows.len())]);
        report.push(json!({
            "k": k,
            "random_f1": rand_f1, "random_acc": rand_acc,
            "rowvar_f1": var_f1, "rowvar_acc": var_acc,
            "measured_f1": meas_f1, "measured_acc": meas_acc,
        }));
    }
    // 5) kill-switch 攻击：把实测 top-1 行**整行置零**（probe 已显示该行
    //    承载全部输出偏移）。edits 数 = 该行非零 trit 数。
    let (top_delta, top_li, top_row) = measured[0];
    let cols = artifact.layers()[top_li].cols();
    let edits: Vec<Edit> = (0..cols)
        .filter(|&c| artifact.layers()[top_li].trits()[top_row * cols + c].to_i8() != 0)
        .map(|c| Edit::SetTrit {
            layer: top_li,
            row: top_row,
            col: c,
            to: Trit::Zero,
        })
        .collect();
    let edits_n = edits.len();
    let mut killed = artifact.clone();
    Patch::new(edits).apply(&mut killed).expect("in-bounds");
    let killed_outs: Vec<f32> = test
        .iter()
        .map(|&i| artifact_out(&killed, &features[i]))
        .collect();
    let killed_metrics = evaluate(&killed_outs, &test_labels);
    let killed_f1 = killed_metrics.f1();
    let killed_acc = killed_metrics.accuracy();

    let total_trits: usize = artifact.layers().iter().map(|l| l.rows() * l.cols()).sum();
    println!(
        "{}",
        json!({
            "stage": "redteam",
            "total_trits": total_trits,
            "clean_f1": clean.f1(),
            "clean_acc": clean.accuracy(),
            "attacks": report,
            "kill_switch": {
                "layer": top_li,
                "row": top_row,
                "probe_delta": top_delta,
                "edits": edits_n,
                "f1": killed_f1,
                "acc": killed_acc,
            },
        })
    );
}
